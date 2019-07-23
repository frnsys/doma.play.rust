extern crate chrono;
extern crate md5;
extern crate noise;
extern crate pbr;
extern crate rand;
extern crate redis;
extern crate serde;
extern crate serde_json;
extern crate serde_yaml;

mod agent;
mod city;
mod config;
mod design;
mod grid;
mod play;
mod sim;
mod stats;
mod sync;
use self::config::Config;
use self::sim::Simulation;
use self::play::PlayManager;
use pbr::ProgressBar;
use rand::rngs::StdRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use chrono::{DateTime, Utc, Local};

fn save_run_data(sim: &Simulation, history: &Vec<Value>, conf: &Config) {
    let now: DateTime<Utc> = Utc::now();
    let now_str = now.format("%Y.%m.%d.%H.%M.%S").to_string();
    let results = json!({
        "history": history,
        "meta": {
            "seed": conf.seed,
            "design": conf.sim.design_id,
            "tenants": sim.tenants.len(),
            "units": sim.city.units.len(),
            "occupancy": sim.city.units.iter().fold(0, |acc, u| acc + u.occupancy)
        }
    })
    .to_string();

    let dir = format!("runs/{}", now_str);
    let fname = format!("runs/{}/output.json", now_str);

    let path = Path::new(&dir);
    let run_path = Path::new(&now_str);
    let latest_path = Path::new("runs/latest");
    fs::create_dir(path).unwrap();
    fs::write(fname, results).expect("Unable to write file");
    if latest_path.exists() {
        fs::remove_file(latest_path).unwrap();
    }
    symlink(run_path, latest_path).unwrap();

    let conf_path = Path::join(path, Path::new("config.yaml"));
    fs::copy(Path::new("config.yaml"), conf_path).unwrap();
    println!("Wrote output to {:?}", path);
}

fn main() {
    let conf = config::load_config();
    let debug = conf.debug;
    let steps = if debug {
        conf.steps
    } else {
        conf.play.turn_sequence.iter().fold(0, |acc, steps| acc + steps) + conf.play.burn_in
    };
    let mut rng: StdRng = SeedableRng::seed_from_u64(conf.seed);

    let mut play = PlayManager::new();
    play.reset().unwrap();
    play.set_loading().unwrap();

    loop {
        let mut turn_sequence = conf.play.turn_sequence.clone();
        let mut switch_step = if debug {
            conf.steps
        } else {
            conf.play.burn_in + turn_sequence.remove(0)
        };

        // Load and setup world
        let design = design::load_design(&conf.sim.design_id);
        let mut sim = Simulation::new(design, &conf.sim, &mut rng);
        println!("{:?} tenants", sim.tenants.len());

        let mut fastfw = false;
        let mut started = false;
        let mut history = Vec::with_capacity(steps);
        let mut pb = ProgressBar::new(steps as u64);

        if !debug {
            // Setup tenants for players to choose
            play.gen_player_tenant_pool(&sim.tenants);
            play.set_ready().unwrap();
            println!("Ready: Session {:?}", Local::now().to_rfc3339());
        }

        for step in 0..steps {
            let burn_in = step < conf.play.burn_in;
            if debug || fastfw || burn_in || play.all_players_ready(&mut sim, started) {
                if !debug {
                    play.sync_step(step, steps).unwrap();
                    play.process_commands(&mut sim).unwrap();
                }

                sim.step(step, &mut rng, &conf.sim);

                // Fast forwarding into the future
                if !debug {
                    if step >= switch_step {
                        switch_step = step + turn_sequence.remove(0);
                        fastfw = !fastfw;
                    }
                    if fastfw {
                        println!("Fast forwarding...");
                        play.set_fast_forward().unwrap();
                        // play.release_player_tenants(&mut sim.tenants);
                    } else if burn_in {
                        println!("Burn in...");
                    } else {
                        started = true;
                        println!("Normal speed...");
                        play.set_in_progress().unwrap();
                    }

                    sync::sync(step, &sim.city, &sim.design, stats::stats(&sim)).unwrap();
                    play.sync_players(&sim.tenants, &sim.city).unwrap();
                    play.reset_ready_players().unwrap();

                    // if !fastfw && !burn_in {
                    //     play.wait_turn(conf.play.min_step_delay);
                    // }
                } else {
                    history.push(stats::stats(&sim));
                }

                pb.inc();
            }
        }
        // End of run

        if debug {
            save_run_data(&sim, &history, &conf);

            // If debug, run only once
            break;
        } else {
            // Wait between runs
            play.set_finished().unwrap();
            play.wait(conf.play.pause_between_runs);
        }
    }
}
