//! Chat messages every Hypolia server shows the same way.

use crate::{LevelProgress, PlayerReward};

/// Command a player runs on a game server to queue for another match.
pub const REPLAY_COMMAND: &str = "/rejouer";

const RULE: &str = "§8§m                                        ";
const BAR_CELLS: u64 = 10;

pub fn join_message(player_name: &str) -> String {
    format!("§8[§a+§8] §7{player_name}")
}

pub fn quit_message(player_name: &str) -> String {
    format!("§8[§c-§8] §7{player_name}")
}

/// `■■■■□□□□□□ 40%`, the share of the current level already earned.
pub fn progress_bar(progress: &LevelProgress) -> String {
    let next = progress.xp_for_next.max(1);
    let into = progress.xp_into_level.min(next);
    let filled = into * BAR_CELLS / next;
    let percent = into * 100 / next;
    let full = "■".repeat(usize::try_from(filled).unwrap_or(0));
    let empty = "■".repeat(usize::try_from(BAR_CELLS - filled).unwrap_or(0));
    format!("§a{full}§7{empty} §f{percent}%")
}

/// End-of-match recap: rewards, then game and network level progress.
pub fn reward_recap(game_name: &str, reward: &PlayerReward) -> Vec<String> {
    let mut lines = vec![
        RULE.to_owned(),
        format!("§6§lRÉCAP DE PARTIE §8• §f{game_name}"),
        format!("§a+{} XP §8• §6+{} coins", reward.xp, reward.coins),
        format!(
            "§7Niveau {game_name} §b{} {}",
            reward.game.level,
            progress_bar(&reward.game)
        ),
        format!(
            "§7Niveau réseau §b{} {}",
            reward.network.level,
            progress_bar(&reward.network)
        ),
    ];
    if reward.game_level_up {
        lines.push(format!(
            "§d✦ Niveau {} {game_name} atteint !",
            reward.game.level
        ));
    }
    if reward.network_level_up {
        lines.push(format!(
            "§b✦ Niveau réseau {} atteint !",
            reward.network.level
        ));
    }
    lines.push(RULE.to_owned());
    lines
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn progress(level: u32, into: u64, next: u64) -> LevelProgress {
        LevelProgress {
            level,
            xp_into_level: into,
            xp_for_next: next,
            total_xp: 0,
        }
    }

    fn reward(game_level_up: bool, network_level_up: bool) -> PlayerReward {
        PlayerReward {
            uuid: Uuid::nil(),
            xp: 120,
            coins: 15,
            game: progress(4, 40, 100),
            game_level_up,
            network: progress(12, 0, 500),
            network_level_up,
        }
    }

    #[test]
    fn join_and_quit_differ_only_by_the_sign() {
        assert_eq!(join_message("Alice"), "§8[§a+§8] §7Alice");
        assert_eq!(quit_message("Alice"), "§8[§c-§8] §7Alice");
    }

    #[test]
    fn progress_bar_fills_one_cell_per_tenth() {
        assert_eq!(progress_bar(&progress(1, 40, 100)), "§a■■■■§7■■■■■■ §f40%");
        assert_eq!(progress_bar(&progress(1, 0, 100)), "§a§7■■■■■■■■■■ §f0%");
        assert_eq!(
            progress_bar(&progress(1, 250, 100)),
            "§a■■■■■■■■■■§7 §f100%"
        );
        assert_eq!(progress_bar(&progress(1, 3, 0)), "§a■■■■■■■■■■§7 §f100%");
    }

    #[test]
    fn recap_shows_rewards_and_both_levels() {
        let lines = reward_recap("SheepWars", &reward(false, false));
        assert!(lines.contains(&"§a+120 XP §8• §6+15 coins".to_owned()));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("§7Niveau SheepWars §b4 "))
        );
        assert!(lines.iter().any(|l| l.starts_with("§7Niveau réseau §b12 ")));
        assert!(!lines.iter().any(|l| l.contains("atteint")));
        assert_eq!(lines.first(), lines.last());
    }

    #[test]
    fn recap_celebrates_each_level_up() {
        let lines = reward_recap("TowerWars", &reward(true, true));
        assert!(lines.contains(&"§d✦ Niveau 4 TowerWars atteint !".to_owned()));
        assert!(lines.contains(&"§b✦ Niveau réseau 12 atteint !".to_owned()));
    }
}
