use std::future::Future;

use thiserror::Error;

use crate::domain::{Bundle, BundleError, Game, MapId, MapName};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("map {0} introuvable")]
    NotFound(MapId),
    #[error("map {id} corrompue dans le stockage : {source}")]
    Corrupt {
        id: MapId,
        #[source]
        source: BundleError,
    },
    #[error(transparent)]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

/// `put` écrit `world.zip` avant `arena.json` : une map n'est listée qu'une fois complète.
pub trait MapStore {
    fn list(&self, game: Game) -> impl Future<Output = Result<Vec<MapName>, StoreError>> + Send;

    fn put(
        &self,
        id: &MapId,
        bundle: &Bundle,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn get(&self, id: &MapId) -> impl Future<Output = Result<Bundle, StoreError>> + Send;

    fn delete(&self, id: &MapId) -> impl Future<Output = Result<(), StoreError>> + Send;
}
