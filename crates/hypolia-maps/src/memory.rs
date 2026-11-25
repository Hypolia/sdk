use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use crate::domain::{Bundle, Game, MapId, MapName, complete_maps};
use crate::store::{MapStore, StoreError};

#[derive(Debug, Default)]
pub struct MemoryMapStore {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<String, Vec<u8>>,
    writes: Vec<String>,
}

impl MemoryMapStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn writes(&self) -> Vec<String> {
        self.lock().writes.clone()
    }

    pub fn insert_raw(&self, key: impl Into<String>, bytes: Vec<u8>) {
        self.lock().objects.insert(key.into(), bytes);
    }

    pub fn keys(&self) -> Vec<String> {
        self.lock().objects.keys().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl MapStore for MemoryMapStore {
    async fn list(&self, game: Game) -> Result<Vec<MapName>, StoreError> {
        let state = self.lock();
        Ok(complete_maps(
            state.objects.keys().map(String::as_str),
            game,
        ))
    }

    async fn put(&self, id: &MapId, bundle: &Bundle) -> Result<(), StoreError> {
        let mut state = self.lock();
        for (key, bytes) in [
            (id.world_key(), bundle.world_zip()),
            (id.arena_key(), bundle.arena_json()),
        ] {
            state.writes.push(key.clone());
            state.objects.insert(key, bytes.to_vec());
        }
        Ok(())
    }

    async fn get(&self, id: &MapId) -> Result<Bundle, StoreError> {
        let state = self.lock();
        let (Some(world), Some(arena)) = (
            state.objects.get(&id.world_key()),
            state.objects.get(&id.arena_key()),
        ) else {
            return Err(StoreError::NotFound(id.clone()));
        };
        Bundle::new(world.clone(), arena.clone()).map_err(|source| StoreError::Corrupt {
            id: id.clone(),
            source,
        })
    }

    async fn delete(&self, id: &MapId) -> Result<(), StoreError> {
        let mut state = self.lock();
        state.objects.remove(&id.world_key());
        state.objects.remove(&id.arena_key());
        Ok(())
    }
}
