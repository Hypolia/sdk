//! Chat messages every Hypolia server shows the same way.

pub fn join_message(player_name: &str) -> String {
    format!("§8[§a+§8] §7{player_name}")
}

pub fn quit_message(player_name: &str) -> String {
    format!("§8[§c-§8] §7{player_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_and_quit_differ_only_by_the_sign() {
        assert_eq!(join_message("Alice"), "§8[§a+§8] §7Alice");
        assert_eq!(quit_message("Alice"), "§8[§c-§8] §7Alice");
    }
}
