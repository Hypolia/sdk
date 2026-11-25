use std::collections::BTreeMap;
use std::fmt;
use std::io::Cursor;
use std::str::FromStr;

use thiserror::Error;

pub const WORLD_FILE: &str = "world.zip";
pub const ARENA_FILE: &str = "arena.json";
const MAP_NAME_MAX_LEN: usize = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Game {
    Sheepwars,
    Towerwars,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Sheepwars, Game::Towerwars];

    pub fn as_str(self) -> &'static str {
        match self {
            Game::Sheepwars => "sheepwars",
            Game::Towerwars => "towerwars",
        }
    }

    pub fn prefix(self) -> String {
        format!("{}/", self.as_str())
    }
}

impl fmt::Display for Game {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("jeu inconnu « {0} », attendu : sheepwars ou towerwars")]
pub struct UnknownGame(String);

impl FromStr for Game {
    type Err = UnknownGame;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Game::ALL
            .into_iter()
            .find(|game| game.as_str() == raw)
            .ok_or_else(|| UnknownGame(raw.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapName(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvalidMapName {
    #[error("nom de map vide")]
    Empty,
    #[error("nom de map trop long ({0} caractères, maximum {MAP_NAME_MAX_LEN})")]
    TooLong(usize),
    #[error("nom de map « {0} » invalide : seuls lettres, chiffres, - et _ sont acceptés")]
    ForbiddenChar(String),
}

impl MapName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn check(raw: &str) -> Result<(), InvalidMapName> {
        if raw.is_empty() {
            return Err(InvalidMapName::Empty);
        }
        if raw.len() > MAP_NAME_MAX_LEN {
            return Err(InvalidMapName::TooLong(raw.len()));
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(InvalidMapName::ForbiddenChar(raw.to_owned()));
        }
        Ok(())
    }
}

impl TryFrom<String> for MapName {
    type Error = InvalidMapName;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::check(&raw)?;
        Ok(Self(raw))
    }
}

impl TryFrom<&str> for MapName {
    type Error = InvalidMapName;

    fn try_from(raw: &str) -> Result<Self, Self::Error> {
        Self::check(raw)?;
        Ok(Self(raw.to_owned()))
    }
}

impl FromStr for MapName {
    type Err = InvalidMapName;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::try_from(raw)
    }
}

impl fmt::Display for MapName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapId {
    pub game: Game,
    pub name: MapName,
}

impl MapId {
    pub fn new(game: Game, name: MapName) -> Self {
        Self { game, name }
    }

    pub fn key(&self, file: &str) -> String {
        format!("{}/{}/{file}", self.game, self.name)
    }

    pub fn world_key(&self) -> String {
        self.key(WORLD_FILE)
    }

    pub fn arena_key(&self) -> String {
        self.key(ARENA_FILE)
    }
}

impl fmt::Display for MapId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.game, self.name)
    }
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("world.zip illisible : {0}")]
    UnreadableZip(#[source] zip::result::ZipError),
    #[error("world.zip ne contient ni world/level.dat ni level.dat à la racine")]
    MissingLevelDat,
    #[error("arena.json n'est pas du JSON valide : {0}")]
    InvalidArena(#[source] serde_json::Error),
    #[error("arena.json doit contenir un objet JSON à la racine")]
    ArenaNotObject,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Bundle {
    world_zip: Vec<u8>,
    arena_json: Vec<u8>,
}

impl Bundle {
    pub fn new(world_zip: Vec<u8>, arena_json: Vec<u8>) -> Result<Self, BundleError> {
        check_world_zip(&world_zip)?;
        check_arena_json(&arena_json)?;
        Ok(Self {
            world_zip,
            arena_json,
        })
    }

    pub fn world_zip(&self) -> &[u8] {
        &self.world_zip
    }

    pub fn arena_json(&self) -> &[u8] {
        &self.arena_json
    }
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("world_zip_bytes", &self.world_zip.len())
            .field("arena_json_bytes", &self.arena_json.len())
            .finish()
    }
}

fn check_world_zip(bytes: &[u8]) -> Result<(), BundleError> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(BundleError::UnreadableZip)?;
    let has_level_dat = ["level.dat", "world/level.dat"]
        .into_iter()
        .any(|name| archive.index_for_name(name).is_some());
    if has_level_dat {
        Ok(())
    } else {
        Err(BundleError::MissingLevelDat)
    }
}

fn check_arena_json(bytes: &[u8]) -> Result<(), BundleError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(BundleError::InvalidArena)?;
    if value.is_object() {
        Ok(())
    } else {
        Err(BundleError::ArenaNotObject)
    }
}

pub fn complete_maps<'a>(keys: impl IntoIterator<Item = &'a str>, game: Game) -> Vec<MapName> {
    let prefix = game.prefix();
    let mut seen: BTreeMap<&str, (bool, bool)> = BTreeMap::new();
    for key in keys {
        let Some((name, file)) = key
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let entry = seen.entry(name).or_default();
        match file {
            WORLD_FILE => entry.0 = true,
            ARENA_FILE => entry.1 = true,
            _ => {}
        }
    }
    seen.into_iter()
        .filter(|(_, (world, arena))| *world && *arena)
        .filter_map(|(name, _)| MapName::try_from(name).ok())
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    pub(crate) fn zip_with(entries: &[&str]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for entry in entries {
            writer
                .start_file(*entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"data").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    pub(crate) fn valid_bundle() -> Bundle {
        Bundle::new(
            zip_with(&["world/level.dat"]),
            b"{\"prebuilt\":true}".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn map_name_rejects_invalid_values() {
        assert_eq!(MapName::try_from(""), Err(InvalidMapName::Empty));
        assert_eq!(
            MapName::try_from("a".repeat(64).as_str()),
            Err(InvalidMapName::TooLong(64))
        );
        assert!(matches!(
            MapName::try_from("pirate/../x"),
            Err(InvalidMapName::ForbiddenChar(_))
        ));
        assert!(matches!(
            MapName::try_from("épée"),
            Err(InvalidMapName::ForbiddenChar(_))
        ));
        assert_eq!(
            MapName::try_from("sky_island-2").unwrap().as_str(),
            "sky_island-2"
        );
        assert!(MapName::try_from("a".repeat(63).as_str()).is_ok());
    }

    #[test]
    fn game_parses_known_names_only() {
        assert_eq!("towerwars".parse::<Game>(), Ok(Game::Towerwars));
        assert!("bedwars".parse::<Game>().is_err());
    }

    #[test]
    fn map_id_builds_bucket_keys() {
        let id = MapId::new(Game::Towerwars, "imperial".parse().unwrap());
        assert_eq!(id.world_key(), "towerwars/imperial/world.zip");
        assert_eq!(id.arena_key(), "towerwars/imperial/arena.json");
    }

    #[test]
    fn bundle_accepts_both_zip_layouts() {
        for layout in ["world/level.dat", "level.dat"] {
            assert!(Bundle::new(zip_with(&[layout]), b"{}".to_vec()).is_ok());
        }
    }

    #[test]
    fn bundle_rejects_invalid_inputs() {
        assert!(matches!(
            Bundle::new(zip_with(&["world/region/r.0.0.mca"]), b"{}".to_vec()),
            Err(BundleError::MissingLevelDat)
        ));
        assert!(matches!(
            Bundle::new(b"not a zip".to_vec(), b"{}".to_vec()),
            Err(BundleError::UnreadableZip(_))
        ));
        assert!(matches!(
            Bundle::new(zip_with(&["level.dat"]), b"{oops".to_vec()),
            Err(BundleError::InvalidArena(_))
        ));
        assert!(matches!(
            Bundle::new(zip_with(&["level.dat"]), b"[1,2]".to_vec()),
            Err(BundleError::ArenaNotObject)
        ));
    }

    #[test]
    fn complete_maps_keeps_maps_with_both_files_sorted() {
        let keys = [
            "towerwars/steampunk/world.zip",
            "towerwars/steampunk/arena.json",
            "towerwars/pirate/world.zip",
            "towerwars/sky_island/arena.json",
            "towerwars/sky_island/world.zip",
            "towerwars/nested/sub/world.zip",
            "towerwars/bad name/world.zip",
            "towerwars/bad name/arena.json",
            "sheepwars/aztec/world.zip",
            "sheepwars/aztec/arena.json",
        ];
        let names: Vec<String> = complete_maps(keys, Game::Towerwars)
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(names, ["sky_island", "steampunk"]);
    }
}
