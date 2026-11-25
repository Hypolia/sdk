use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PARTY_MAX_SIZE: usize = 4;
pub const MAX_FRIENDS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRef {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendsList {
    pub friends: Vec<PlayerRef>,
    pub incoming: Vec<PlayerRef>,
    pub outgoing: Vec<PlayerRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendRequest {
    pub player: PlayerRef,
    pub target_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FriendOutcome {
    RequestSent { target: PlayerRef },
    Accepted { friend: PlayerRef },
    Denied { target: PlayerRef },
    Removed { target: PlayerRef },
    AlreadyFriends { target: PlayerRef },
    AlreadyRequested { target: PlayerRef },
    NoRequest,
    NotFriends,
    UnknownPlayer,
    Yourself,
    TooManyFriends,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub id: Uuid,
    pub leader: PlayerRef,
    pub members: Vec<PlayerRef>,
    pub invited: Vec<PlayerRef>,
}

impl Party {
    pub fn contains(&self, player: Uuid) -> bool {
        self.members.iter().any(|member| member.uuid == player)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyRequest {
    pub player: PlayerRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PlayerRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PartyOutcome {
    Invited {
        party: Party,
        target: PlayerRef,
    },
    Joined {
        party: Party,
    },
    Left {
        party: Option<Party>,
        player: PlayerRef,
    },
    Kicked {
        party: Party,
        target: PlayerRef,
    },
    Disbanded {
        members: Vec<PlayerRef>,
    },
    NotInParty,
    NotLeader,
    AlreadyInParty,
    TargetInParty,
    PartyFull,
    NoInvite,
    Yourself,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(id: u128, name: &str) -> PlayerRef {
        PlayerRef {
            uuid: Uuid::from_u128(id),
            name: name.to_owned(),
        }
    }

    #[test]
    fn outcomes_are_tagged_in_snake_case() {
        let json = serde_json::to_value(FriendOutcome::RequestSent {
            target: player(1, "Alex"),
        })
        .unwrap();
        assert_eq!(json["outcome"], "request_sent");
        assert_eq!(json["target"]["name"], "Alex");
        let back: FriendOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(
            back,
            FriendOutcome::RequestSent {
                target: player(1, "Alex")
            }
        );
        let json = serde_json::to_value(PartyOutcome::PartyFull).unwrap();
        assert_eq!(json["outcome"], "party_full");
    }

    #[test]
    fn a_party_knows_its_members() {
        let party = Party {
            id: Uuid::from_u128(9),
            leader: player(1, "Steve"),
            members: vec![player(1, "Steve"), player(2, "Alex")],
            invited: Vec::new(),
        };
        assert!(party.contains(Uuid::from_u128(2)));
        assert!(!party.contains(Uuid::from_u128(3)));
    }
}
