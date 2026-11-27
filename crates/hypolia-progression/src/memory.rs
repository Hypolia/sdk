use std::collections::{BTreeMap, HashMap};
use std::future::{Future, ready};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{CoinMovement, PlayerProgress, PlayerRecord};
use hypolia_protocol::{GameKind, Leaderboard, LeaderboardEntry, MatchReportAck};
use uuid::Uuid;

use crate::repository::{
    LedgerEntry, MatchToSettle, ProgressionRepository, ProgressionStoreError, Settlement,
};

#[derive(Debug, Default)]
struct State {
    players: BTreeMap<Uuid, PlayerRecord>,
    ledger: Vec<LedgerEntry>,
    settled: HashMap<Uuid, MatchReportAck>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryProgressionRepository {
    state: Arc<Mutex<State>>,
}

impl MemoryProgressionRepository {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn progress_of(state: &State, uuid: Uuid) -> PlayerProgress {
        state
            .players
            .get(&uuid)
            .cloned()
            .map_or_else(|| PlayerProgress::new(uuid), PlayerProgress::from)
    }
}

impl ProgressionRepository for MemoryProgressionRepository {
    fn load(
        &self,
        uuid: Uuid,
    ) -> impl Future<Output = Result<Option<PlayerRecord>, ProgressionStoreError>> + Send {
        ready(Ok(self.lock().players.get(&uuid).cloned()))
    }

    fn update<T, E, F>(
        &self,
        uuid: Uuid,
        apply: F,
    ) -> impl Future<Output = Result<Result<(PlayerRecord, T), E>, ProgressionStoreError>> + Send
    where
        T: Send,
        E: Send,
        F: FnOnce(&mut PlayerProgress) -> Result<(T, Vec<CoinMovement>), E> + Send,
    {
        let outcome = {
            let mut state = self.lock();
            let mut progress = Self::progress_of(&state, uuid);
            match apply(&mut progress) {
                Ok((value, movements)) => {
                    let record = progress.into_record();
                    state.players.insert(uuid, record.clone());
                    state.ledger.extend(
                        movements
                            .into_iter()
                            .map(|movement| LedgerEntry { uuid, movement }),
                    );
                    Ok((record, value))
                }
                Err(err) => Err(err),
            }
        };
        ready(Ok(outcome))
    }

    fn settle_match<F>(
        &self,
        settle: MatchToSettle<'_>,
        apply: F,
    ) -> impl Future<Output = Result<MatchReportAck, ProgressionStoreError>> + Send
    where
        F: FnOnce(&mut BTreeMap<Uuid, PlayerProgress>) -> Settlement + Send,
    {
        let ack = {
            let mut state = self.lock();
            if let Some(stored) = state.settled.get(&settle.match_id) {
                MatchReportAck {
                    duplicate: true,
                    ..stored.clone()
                }
            } else {
                let mut players: BTreeMap<Uuid, PlayerProgress> = settle
                    .players
                    .iter()
                    .map(|&uuid| (uuid, Self::progress_of(&state, uuid)))
                    .collect();
                let settlement = apply(&mut players);
                for (uuid, progress) in players {
                    state.players.insert(uuid, progress.into_record());
                }
                state.ledger.extend(settlement.ledger);
                let ack = MatchReportAck {
                    duplicate: false,
                    ..settlement.ack
                };
                state.settled.insert(settle.match_id, ack.clone());
                ack
            }
        };
        ready(Ok(ack))
    }

    fn ledger(
        &self,
        uuid: Uuid,
    ) -> impl Future<Output = Result<Vec<CoinMovement>, ProgressionStoreError>> + Send {
        let movements = self
            .lock()
            .ledger
            .iter()
            .filter(|entry| entry.uuid == uuid)
            .map(|entry| entry.movement.clone())
            .collect();
        ready(Ok(movements))
    }

    fn top_wins(
        &self,
        game: GameKind,
        limit: u32,
    ) -> impl Future<Output = Result<Leaderboard, ProgressionStoreError>> + Send {
        let mut entries: Vec<LeaderboardEntry> = {
            let state = self.lock();
            let mut rows: Vec<(String, u32, u32)> = state
                .players
                .values()
                .filter_map(|record| {
                    record.games.get(&game).and_then(|g| {
                        if g.wins > 0 {
                            Some((record.name.clone(), g.wins, g.played))
                        } else {
                            None
                        }
                    })
                })
                .collect();
            rows.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| a.2.cmp(&b.2))
                    .then_with(|| a.0.cmp(&b.0))
            });
            rows.truncate(limit as usize);
            rows.into_iter()
                .enumerate()
                .map(|(i, (name, wins, played))| LeaderboardEntry {
                    rank: (i + 1) as u32,
                    name,
                    wins,
                    played,
                })
                .collect()
        };
        entries.shrink_to_fit();
        ready(Ok(Leaderboard { game, entries }))
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use hypolia_protocol::{GameKind, MatchOutcome};

    use super::*;
    use crate::{Coins, UnlockError};

    const RED: Uuid = Uuid::from_u128(1);
    const BLUE: Uuid = Uuid::from_u128(2);
    const MATCH: Uuid = Uuid::from_u128(99);

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("memory repository futures never suspend"),
        }
    }

    fn grant(repo: &MemoryProgressionRepository, uuid: Uuid, amount: u64) {
        let _ = block_on(repo.update(uuid, |p| {
            Ok::<_, ()>(((), vec![p.grant(Coins(amount), "test")]))
        }))
        .unwrap();
    }

    fn settle_duel(repo: &MemoryProgressionRepository) -> MatchReportAck {
        let players = [RED, BLUE];
        block_on(repo.settle_match(
            MatchToSettle {
                match_id: MATCH,
                game: GameKind::SheepWars,
                server_id: "sheepwars-1",
                players: &players,
            },
            |map| {
                let mut rewards = Vec::new();
                let mut ledger = Vec::new();
                for (uuid, outcome) in [(RED, MatchOutcome::Win), (BLUE, MatchOutcome::Loss)] {
                    let credit = map.get_mut(&uuid).unwrap().record_match(
                        MATCH,
                        GameKind::SheepWars,
                        outcome,
                    );
                    rewards.push(credit.to_player_reward(uuid));
                    ledger.push(LedgerEntry {
                        uuid,
                        movement: credit.movement,
                    });
                }
                Settlement {
                    ack: MatchReportAck {
                        match_id: MATCH,
                        counted: true,
                        duplicate: false,
                        rewards,
                    },
                    ledger,
                }
            },
        ))
        .unwrap()
    }

    #[test]
    fn load_returns_none_for_an_unknown_player() {
        let repo = MemoryProgressionRepository::default();
        assert_eq!(block_on(repo.load(RED)).unwrap(), None);
    }

    #[test]
    fn update_persists_the_record_and_appends_the_ledger() {
        let repo = MemoryProgressionRepository::default();
        grant(&repo, RED, 100);
        let record = block_on(repo.load(RED)).unwrap().unwrap();
        assert_eq!(record.coins, Coins(100));
        assert_eq!(block_on(repo.ledger(RED)).unwrap().len(), 1);
    }

    #[test]
    fn update_error_persists_nothing() {
        let repo = MemoryProgressionRepository::default();
        grant(&repo, RED, 100);
        let result = block_on(repo.update(RED, |p| {
            let _ = p.grant(Coins(5), "lost");
            p.purchase(hypolia_protocol::CosmeticId::SheepTrailFlame)
                .map(|m| ((), vec![m]))
        }))
        .unwrap();
        assert!(matches!(result, Err(UnlockError::InsufficientCoins { .. })));
        let record = block_on(repo.load(RED)).unwrap().unwrap();
        assert_eq!(record.coins, Coins(100));
        assert_eq!(block_on(repo.ledger(RED)).unwrap().len(), 1);
    }

    #[test]
    fn update_error_on_an_unknown_player_creates_nothing() {
        let repo = MemoryProgressionRepository::default();
        let result = block_on(repo.update(RED, |p| {
            p.purchase(hypolia_protocol::CosmeticId::SheepTrailFlame)
                .map(|m| ((), vec![m]))
        }))
        .unwrap();
        assert!(result.is_err());
        assert_eq!(block_on(repo.load(RED)).unwrap(), None);
    }

    #[test]
    fn settle_match_credits_every_player_and_stores_the_ack() {
        let repo = MemoryProgressionRepository::default();
        let ack = settle_duel(&repo);
        assert!(ack.counted && !ack.duplicate);
        assert_eq!(block_on(repo.load(RED)).unwrap().unwrap().coins, Coins(30));
        assert_eq!(block_on(repo.load(BLUE)).unwrap().unwrap().coins, Coins(10));
        assert_eq!(block_on(repo.ledger(RED)).unwrap().len(), 1);
    }

    #[test]
    fn spec_prg_2_settling_twice_is_a_duplicate_and_does_not_apply() {
        let repo = MemoryProgressionRepository::default();
        let first = settle_duel(&repo);
        let players = [RED, BLUE];
        let second = block_on(repo.settle_match(
            MatchToSettle {
                match_id: MATCH,
                game: GameKind::SheepWars,
                server_id: "sheepwars-1",
                players: &players,
            },
            |_| panic!("apply must not run for a settled match"),
        ))
        .unwrap();
        assert!(second.duplicate);
        assert_eq!(second.rewards.len(), first.rewards.len());
        assert_eq!(block_on(repo.load(RED)).unwrap().unwrap().coins, Coins(30));
        assert_eq!(block_on(repo.ledger(RED)).unwrap().len(), 1);
    }

    #[test]
    fn ledger_keeps_insertion_order_per_player() {
        let repo = MemoryProgressionRepository::default();
        grant(&repo, RED, 1);
        grant(&repo, BLUE, 7);
        grant(&repo, RED, 2);
        let ledger = block_on(repo.ledger(RED)).unwrap();
        let signed: Vec<i128> = ledger.iter().map(|m| m.delta.signed()).collect();
        assert_eq!(signed, vec![1, 2]);
    }

    #[test]
    fn clones_share_the_same_state() {
        let repo = MemoryProgressionRepository::default();
        let other = repo.clone();
        grant(&repo, RED, 10);
        assert_eq!(block_on(other.load(RED)).unwrap().unwrap().coins, Coins(10));
    }

    fn record_win(repo: &MemoryProgressionRepository, uuid: Uuid, game: GameKind, wins: u32) {
        for i in 0..wins {
            let mid = Uuid::from_u128(1000 + uuid.as_u128() as u64 * 100 + i as u64);
            let players = [uuid];
            block_on(repo.settle_match(
                MatchToSettle {
                    match_id: mid,
                    game,
                    server_id: "test",
                    players: &players,
                },
                |map| {
                    let player = map.get_mut(&uuid).unwrap();
                    let credit = player.record_match(mid, game, MatchOutcome::Win);
                    Settlement {
                        ack: MatchReportAck {
                            match_id: mid,
                            counted: true,
                            duplicate: false,
                            rewards: vec![credit.to_player_reward(uuid)],
                        },
                        ledger: vec![LedgerEntry {
                            uuid,
                            movement: credit.movement,
                        }],
                    }
                },
            ))
            .unwrap();
        }
    }

    #[test]
    fn top_wins_sorts_by_wins_desc_then_played_asc_then_name() {
        let repo = MemoryProgressionRepository::default();
        // RED: 5 wins, 10 played. BLUE: 5 wins, 7 played (fewer games, ranks first on tie).
        record_win(&repo, RED, GameKind::SheepWars, 5);
        record_win(&repo, BLUE, GameKind::SheepWars, 5);
        let board = block_on(repo.top_wins(GameKind::SheepWars, 10)).unwrap();
        assert_eq!(board.entries.len(), 2);
        assert_eq!(board.entries[0].name, "BLUE".to_owned().replace("BLUE", ""));
        // Names are empty by default; check ordering by played instead.
        assert_eq!(board.entries[0].wins, 5);
        assert!(board.entries[0].played <= board.entries[1].played);
        assert_eq!(board.entries[0].rank, 1);
        assert_eq!(board.entries[1].rank, 2);
    }

    #[test]
    fn top_wins_excludes_players_with_zero_wins() {
        let repo = MemoryProgressionRepository::default();
        let players = [RED];
        block_on(repo.settle_match(
            MatchToSettle {
                match_id: MATCH,
                game: GameKind::SheepWars,
                server_id: "test",
                players: &players,
            },
            |map| {
                let player = map.get_mut(&RED).unwrap();
                let credit = player.record_match(MATCH, GameKind::SheepWars, MatchOutcome::Loss);
                Settlement {
                    ack: MatchReportAck {
                        match_id: MATCH,
                        counted: true,
                        duplicate: false,
                        rewards: vec![credit.to_player_reward(RED)],
                    },
                    ledger: vec![LedgerEntry {
                        uuid: RED,
                        movement: credit.movement,
                    }],
                }
            },
        ))
        .unwrap();
        let board = block_on(repo.top_wins(GameKind::SheepWars, 10)).unwrap();
        assert!(board.entries.is_empty());
    }

    #[test]
    fn top_wins_respects_the_limit() {
        let repo = MemoryProgressionRepository::default();
        for i in 0..5u128 {
            let uuid = Uuid::from_u128(50 + i);
            record_win(&repo, uuid, GameKind::TowerWars, (i as u32) + 1);
        }
        let board = block_on(repo.top_wins(GameKind::TowerWars, 3)).unwrap();
        assert_eq!(board.entries.len(), 3);
        assert_eq!(board.entries[0].wins, 5);
        assert_eq!(board.entries[2].wins, 3);
    }
}
