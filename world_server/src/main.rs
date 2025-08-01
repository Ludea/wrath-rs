use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use async_ctrlc::CtrlC;
use client_manager::ClientManager;
use packet_handler::{PacketHandler, PacketToHandle};
use time::macros::format_description;
use futures::future::{select, Either};
use futures::pin_mut;
use futures_timer::Delay;
use smol_macros::main;
use macro_rules_attribute::apply;
use tracing_subscriber::{fmt::time::UtcTime, EnvFilter};
use wrath_auth_db::AuthDatabase;
use wrath_realm_db::RealmDatabase;

mod auth;
mod character;
mod client;
mod client_manager;
mod console_input;
mod constants;
mod data;
pub mod handlers;
mod item;
mod packet;
mod packet_handler;
mod world;

pub mod prelude {
    pub use super::handlers;
    pub use anyhow::{anyhow, bail, Result};
    pub use tracing::{error, info, trace, warn};
    pub use wow_world_messages::Guid;
}
use prelude::*;

#[apply(main!)]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let timer = UtcTime::new(format_description!("[day]-[month]-[year] [hour]:[minute]:[second]"));
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(EnvFilter::new("wrath=info,sqlx=warn"))
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(timer)
        .init();

    info!("Starting World Server");
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let ctrlc = CtrlC::new().expect("Failed to register ctrl+c abort handler");
    let _ = smol::spawn(async move {
        ctrlc.await;
        info!("Detected Ctrl+C, starting graceful shutdown");
        r.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    let db_connect_timeout = Duration::from_secs(std::env::var("DB_CONNECT_TIMEOUT_SECONDS")?.parse()?);
    let auth_database = AuthDatabase::new(&std::env::var("AUTH_DATABASE_URL")?, db_connect_timeout).await?;
    let auth_database_ref = std::sync::Arc::new(auth_database);

    let realm_database = RealmDatabase::new(&std::env::var("REALM_DATABASE_URL")?, db_connect_timeout).await?;
    let realm_database_ref = std::sync::Arc::new(realm_database);

    let mut data_storage = data::DataStorage::default();
    data_storage.load(realm_database_ref.clone()).await?;
    let data_storage = std::sync::Arc::new(data_storage);

    let _ = smol::spawn(auth::auth_server_heartbeats());

    let world = std::sync::Arc::new(world::World::new(realm_database_ref));

    let (sender, receiver) = std::sync::mpsc::channel::<PacketToHandle>();
    let realm_packet_handler = PacketHandler::new(receiver, world.clone());

    let client_manager = std::sync::Arc::new(ClientManager::new(auth_database_ref.clone(), data_storage));
    let client_manager_for_acceptloop = client_manager.clone();

    let _ = smol::spawn(async move {
        client_manager_for_acceptloop
            .accept_realm_connections(sender)
            .await
            .unwrap_or_else(|e| warn!("Error in realm_socket::accept_realm_connections: {:?}", e))
    });

    let _ = smol::spawn(console_input::process_console_commands(running.clone()));

    let desired_timestep_sec: f32 = 1.0 / 10.0;
    let mut previous_loop_total: f32 = desired_timestep_sec;

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        let before = std::time::Instant::now();
        client_manager.tick(previous_loop_total, world.clone()).await.unwrap_or_else(|e| {
            error!("Error while ticking clients: {}", e);
        });
        realm_packet_handler.handle_queue(client_manager.clone(), world.clone()).await?;
        #[cfg(debug_assertions)]
        {
            let tick_fut = world.tick(previous_loop_total);
	    let timeout_fut = Delay::new(Duration::from_secs_f32(10.0));
	    pin_mut!(tick_fut);
	    pin_mut!(timeout_fut);

	    match select(tick_fut, timeout_fut).await {
    		Either::Left((result, _timeout)) => { 
        	  result?;
		}
    Either::Right((_timeout, _unfinished_tick)) => {
        panic!("deadlock: tick timeout");
    }}
   }   
        #[cfg(not(debug_assertions))]
        {
            world.tick(previous_loop_total).await?;
        }
        let after = std::time::Instant::now();
        let update_duration = after.duration_since(before);
        if update_duration.as_secs_f32() < desired_timestep_sec {
            async_io::Timer::after(std::time::Duration::from_secs_f32(desired_timestep_sec - update_duration.as_secs_f32())).await;
        } else {
            warn!("Too long tick to keep up with desired timestep!");
        }
        previous_loop_total = std::time::Instant::now().duration_since(before).as_secs_f32();
    }

    info!("World server shut down");
    Ok(())
}
