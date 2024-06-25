use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    thread,
};

use flume::{Receiver, Sender};
use noise::NoiseFn;
use serde::{Deserialize, Serialize};
use valence::prelude::*;

use noise_builder::{DynNoise, NoiseBuilder};

pub mod noise_builder;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                remove_unviewed_chunks,
                update_client_views,
                send_recv_chunks,
            )
                .chain(),
        );
    }
}

pub struct TerrainGenConfig {
    pub block: BlockState,
    pub surface_layers: Vec<(u16, BlockState)>,
    pub noise: NoiseBuilder,
    pub height: u32,
    //TODO impl structures
}

impl Default for TerrainGenConfig {
    fn default() -> Self {
        Self {
            block: BlockState::DIRT,
            surface_layers: vec![(1, BlockState::GRASS_BLOCK)],
            noise: NoiseBuilder::Constant(64.0),
            height: 384,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SerializableTerrainGenConfig {
    pub block: String,
    pub surface_layers: Vec<(u16, String)>,
    pub noise: String,
    pub height: u32,
}

impl SerializableTerrainGenConfig {
    pub fn parse(self) -> Result<TerrainGenConfig, String> {
        let mut surface_layers = vec![];
        for (amt, layer) in self.surface_layers {
            surface_layers.push(match block_from_str(&layer) {
                Ok(block) => (amt, block),
                Err(e) => return Err(e),
            });
        }
        Ok(TerrainGenConfig {
            block: block_from_str(&self.block)?,
            surface_layers,
            noise: NoiseBuilder::parse(&self.noise)?,
            height: self.height,
        })
    }
}

fn block_from_str(s: &str) -> Result<BlockState, String> {
    match BlockKind::from_str(&s) {
        Some(block) => Ok(BlockState::from_kind(block)),
        None => Err(format!("Invalid block: '{}'", s)),
    }
}

struct ChunkWorkerState {
    block: BlockState,
    surface_layers: Vec<(u16, BlockState)>,
    noise: DynNoise,
    sender: Sender<(ChunkPos, UnloadedChunk)>,
    receiver: Receiver<ChunkPos>,
    height: u32,
}

#[derive(Component)]
pub struct TerrainGenerator {
    /// Chunks that need to be generated. Chunks without a priority have already
    /// been sent to the thread pool.
    pending: HashMap<ChunkPos, Option<u64>>,
    sender: Sender<ChunkPos>,
    receiver: Receiver<(ChunkPos, UnloadedChunk)>,
    render_dist: u8,
    needs_reload: bool,
}

impl TerrainGenerator {
    /// Set render_dist to 0 to always use client render distance
    pub fn new(config: TerrainGenConfig, render_dist: u8) -> Self {
        let (finished_sender, finished_receiver) = flume::unbounded();
        let (pending_sender, pending_receiver) = flume::unbounded();
        let state = Arc::new(ChunkWorkerState {
            block: config.block,
            surface_layers: config.surface_layers,
            noise: config.noise.build(),
            sender: finished_sender,
            receiver: pending_receiver,
            height: config.height,
        });
        for _ in 0..thread::available_parallelism().unwrap().get() {
            let state = state.clone();
            thread::spawn(move || chunk_worker(state));
        }
        Self {
            pending: HashMap::new(),
            sender: pending_sender,
            receiver: finished_receiver,
            needs_reload: true,
            render_dist,
        }
    }

    pub fn render_dist(&self) -> u8 {
        return self.render_dist;
    }

    pub fn set_render_dist(&mut self, dist: u8) {
        self.render_dist = dist;
        self.needs_reload = true;
    }

    pub fn reload(&mut self, config: TerrainGenConfig) {
        *self = Self::new(config, self.render_dist);
    }
}

fn remove_unviewed_chunks(mut layers: Query<&mut ChunkLayer, With<TerrainGenerator>>) {
    for mut layer in layers.iter_mut() {
        layer.retain_chunks(|_, chunk| chunk.viewer_count_mut() > 0)
    }
}

fn update_client_views(
    mut layers: Query<(&mut ChunkLayer, &mut TerrainGenerator)>,
    mut clients: Query<(&mut Client, View, OldView, &VisibleChunkLayer)>,
) {
    for (client, view, old_view, visible_layer) in &mut clients {
        let (layer, mut terrain_gen) = match layers.get_mut(visible_layer.0) {
            Ok(v) => v,
            // not in layer with terrain gen, move on
            Err(_) => continue,
        };

        let mut view = view.get();
        let mut old_view = old_view.get();
        if terrain_gen.render_dist != 0 {
            view = view.with_dist(view.dist().min(terrain_gen.render_dist));
            old_view = old_view.with_dist(view.dist().min(terrain_gen.render_dist));
        }
        let reload = terrain_gen.needs_reload;
        let queue_pos = |pos: ChunkPos| {
            if layer.chunk(pos).is_none() {
                match terrain_gen.pending.entry(pos) {
                    Entry::Occupied(mut oe) => {
                        if let Some(priority) = oe.get_mut() {
                            let dist = view.pos.distance_squared(pos);
                            *priority = (*priority).min(dist);
                        }
                    }
                    Entry::Vacant(ve) => {
                        let dist = view.pos.distance_squared(pos);
                        ve.insert(Some(dist));
                    }
                }
            }
        };

        // Queue all the new chunks in the view to be sent to the thread pool.
        if client.is_added() || reload {
            view.iter().for_each(queue_pos);
        } else if old_view != view {
            view.diff(old_view).for_each(queue_pos);
        }
    }

    for (_, mut terrain_gen) in layers.iter_mut() {
        terrain_gen.needs_reload = false;
    }
}

fn send_recv_chunks(mut layers: Query<(&mut ChunkLayer, &mut TerrainGenerator)>) {
    for (mut layer, mut terrain_gen) in layers.iter_mut() {
        // Insert the chunks that are finished generating into the instance.
        // needs collect to not borrow
        for (pos, chunk) in terrain_gen.receiver.drain().collect::<Vec<_>>() {
            layer.insert_chunk(pos, chunk);
            assert!(terrain_gen.pending.remove(&pos).is_some());
        }
        // Collect all the new chunks that need to be loaded this tick.
        let mut to_send = vec![];

        for (pos, priority) in &mut terrain_gen.pending {
            if let Some(pri) = priority.take() {
                to_send.push((pri, *pos));
            }
        }

        // Sort chunks by ascending priority.
        to_send.sort_unstable_by_key(|(pri, _)| *pri);

        // Send the sorted chunks to be loaded.
        for (_, pos) in to_send {
            let _ = terrain_gen.sender.try_send(pos);
        }
    }
}

fn chunk_worker(state: Arc<ChunkWorkerState>) {
    while let Ok(pos) = state.receiver.recv() {
        let mut chunk = UnloadedChunk::with_height(state.height);
        // pretty sure clone is a good idea to not lock state as much as possible
        let layers = state.surface_layers.clone();
        let block = state.block;
        let surface_height = layers.iter().map(|(a, _)| a).sum::<u16>() as i32;
        for offset_x in 0..16 {
            for offset_z in 0..16 {
                let height = (state.noise.get([
                    (offset_x as i32 + pos.x * 16) as f64,
                    (offset_z as i32 + pos.z * 16) as f64,
                ]) as i32)
                    .clamp(1, chunk.height() as i32 - 1);
                // remaning blocks until change
                let mut rem = height - surface_height;
                // current block index, -1 means not surface
                let mut curidx = -1i32;
                while rem <= 0 {
                    curidx += 1;
                    rem += layers[curidx as usize].0 as i32
                }
                for y in 0..chunk.height() {
                    if rem == 0 {
                        curidx += 1;
                        rem = if (curidx as usize) < layers.len() {
                            layers[curidx as usize].0 as i32
                        } else {
                            0
                        };
                    }
                    rem -= 1;
                    let res_block = if curidx == -1 {
                        block
                    } else if rem < 0 {
                        BlockState::AIR
                    } else {
                        layers[curidx as usize].1
                    };
                    chunk.set_block(offset_x, y, offset_z, res_block);
                }
            }
        }
        let _ = state.sender.send((pos, chunk));
    }
}
