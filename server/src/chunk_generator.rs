use crate::metrics::ChunkGenMetrics;
#[cfg(not(feature = "worldgen"))]
use crate::test_world::{IndexOwned, World};
use common::{
    generation::ChunkSupplement, resources::TimeOfDay, slowjob::SlowJobPool, terrain::TerrainChunk,
};
use hashbrown::{hash_map::Entry, HashMap};
use specs::Entity as EcsEntity;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use vek::*;
#[cfg(feature = "worldgen")]
use world::{IndexOwned, World};

type ChunkGenResult = (
    Vec2<i32>,
    Result<(TerrainChunk, ChunkSupplement), Option<EcsEntity>>,
);

pub struct ChunkGenerator {
    chunk_tx: crossbeam_channel::Sender<ChunkGenResult>,
    chunk_rx: crossbeam_channel::Receiver<ChunkGenResult>,
    pending_chunks: HashMap<Vec2<i32>, Arc<AtomicBool>>,
    metrics: Arc<ChunkGenMetrics>,
}
impl ChunkGenerator {
    #[allow(clippy::new_without_default)] // TODO: Pending review in #587
    pub fn new(metrics: ChunkGenMetrics) -> Self {
        let (chunk_tx, chunk_rx) = crossbeam_channel::unbounded();
        Self {
            chunk_tx,
            chunk_rx,
            pending_chunks: HashMap::new(),
            metrics: Arc::new(metrics),
        }
    }

    pub fn generate_chunk(
        &mut self,
        entity: Option<EcsEntity>,
        key: Vec2<i32>,
        slowjob_pool: &SlowJobPool,
        world: Arc<World>,
        index: IndexOwned,
        time: TimeOfDay,
    ) {
        let v = if let Entry::Vacant(v) = self.pending_chunks.entry(key) {
            v
        } else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        v.insert(Arc::clone(&cancel));
        let chunk_tx = self.chunk_tx.clone();
        self.metrics.chunks_requested.inc();
        slowjob_pool.spawn("CHUNK_GENERATOR", move || {
            let index = index.as_index_ref();
            let payload = world
                .generate_chunk(index, key, || cancel.load(Ordering::Relaxed), Some(time))
                .map_err(|_| entity);
            let _ = chunk_tx.send((key, payload));
        });
    }

    pub fn recv_new_chunk(&mut self) -> Option<ChunkGenResult> {
        // Make sure chunk wasn't cancelled and if it was check to see if there are more
        // chunks to receive
        while let Ok((key, res)) = self.chunk_rx.try_recv() {
            if self.pending_chunks.remove(&key).is_some() {
                self.metrics.chunks_served.inc();
                // TODO: do anything else if res is an Err?
                return Some((key, res));
            }
        }

        None
    }

    pub fn pending_chunks(&self) -> impl Iterator<Item = Vec2<i32>> + '_ {
        self.pending_chunks.keys().copied()
    }

    pub fn cancel_if_pending(&mut self, key: Vec2<i32>) {
        if let Some(cancel) = self.pending_chunks.remove(&key) {
            cancel.store(true, Ordering::Relaxed);
            self.metrics.chunks_canceled.inc();
        }
    }

    pub fn cancel_all(&mut self) {
        let metrics = Arc::clone(&self.metrics);
        self.pending_chunks.drain().for_each(|(_, cancel)| {
            cancel.store(true, Ordering::Relaxed);
            metrics.chunks_canceled.inc();
        });
    }
}
