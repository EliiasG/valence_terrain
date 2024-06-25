use std::fs;

use command::handler::CommandResultEvent;
use command_macros::Command;
use valence::abilities::FlyingSpeed;
use valence::command::scopes::CommandScopes;
use valence::command::{self, AddCommand, CommandScopeRegistry};
use valence::op_level::OpLevel;
use valence::spawn::IsFlat;
use valence::{command_macros, prelude::*};
use valence_terrain::{
    SerializableTerrainGenConfig, TerrainGenConfig, TerrainGenerator, TerrainPlugin,
};

const SPAWN_POS: DVec3 = DVec3::new(0.0, 150.0, 0.0);

#[derive(Command, Debug, Clone)]
#[paths("reload", "rl")]
#[scopes("valence.command.reload")]
struct ReloadCommand;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, TerrainPlugin))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (init_clients, despawn_disconnected_clients, handle_reload),
        )
        .add_command::<ReloadCommand>()
        .add_systems(Update, handle_reload)
        .add_systems(
            Startup,
            |mut command_scopes: ResMut<CommandScopeRegistry>| {
                command_scopes.link("valence.admin", "valence.command")
            },
        )
        .run();
}

fn load_config() -> Result<TerrainGenConfig, String> {
    match fs::read_to_string("terrain.yml") {
        Ok(content) => match serde_yml::from_str::<SerializableTerrainGenConfig>(&content) {
            Ok(cfg) => cfg.parse(),
            Err(e) => Err(format!("error while reading yaml '{}'", e.to_string())),
        },
        Err(e) => Err(format!("error while reading file '{}'", e.to_string())),
    }
}

fn setup(
    mut commands: Commands,
    server: Res<Server>,
    dimensions: Res<DimensionTypeRegistry>,
    biomes: Res<BiomeRegistry>,
) {
    let layer = LayerBundle::new(ident!("overworld"), &dimensions, &biomes, &server);

    commands.spawn((
        layer,
        // server will immediatley crash if wrong config on startup
        TerrainGenerator::new(load_config().expect("error in config"), 0),
    ));
}

fn init_clients(
    mut clients: Query<
        (
            &mut EntityLayerId,
            &mut VisibleChunkLayer,
            &mut VisibleEntityLayers,
            &mut Position,
            &mut GameMode,
            &mut IsFlat,
            &mut FlyingSpeed,
            &mut CommandScopes,
            &mut OpLevel,
        ),
        Added<Client>,
    >,
    layers: Query<Entity, (With<ChunkLayer>, With<EntityLayer>)>,
) {
    for (
        mut layer_id,
        mut visible_chunk_layer,
        mut visible_entity_layers,
        mut pos,
        mut game_mode,
        mut is_flat,
        mut flyspeed,
        mut scopes,
        mut op_level,
    ) in &mut clients
    {
        let layer = layers.single();

        layer_id.0 = layer;
        visible_chunk_layer.0 = layer;
        visible_entity_layers.0.insert(layer);
        pos.set(SPAWN_POS);
        *game_mode = GameMode::Creative;
        op_level.set(4);
        scopes.add("valence.admin");
        is_flat.0 = true;
        flyspeed.0 = 0.5;
    }
}

fn handle_reload(
    mut events: EventReader<CommandResultEvent<ReloadCommand>>,
    mut layer: Query<(&mut ChunkLayer, &mut TerrainGenerator)>,
    mut client: Query<&mut Client>,
) {
    if events.is_empty() {
        return;
    }
    let (mut layer, mut terrain_gen) = layer.single_mut();
    let mut client = match client.get_mut(events.read().next().unwrap().executor) {
        Ok(client) => client,
        Err(_) => return,
    };
    events.clear();
    layer.clear_chunks();
    let config = match crate::load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            client.send_chat_message(format!("error while loading terrain: {}", e));
            return;
        }
    };
    terrain_gen.reload(config);
}
