//! Prints the fans and every temperature key.
//!
//!   cargo run -p utilify-smc --example fans

use utilify_smc::{FAN_FORCE, Smc, SmcIo, read_fans};

fn main() -> Result<(), utilify_smc::Error> {
    let smc = Smc::open()?;
    println!("{} keys", smc.key_count()?);
    match smc.read_f64(FAN_FORCE) {
        Ok(v) => println!("Ftst = {v}"),
        Err(e) => println!("Ftst: {e}"),
    }
    for fan in read_fans(&smc)? {
        println!(
            "fan {}: {:.0} rpm (target {:.0}, {:.0}..{:.0}){}",
            fan.index,
            fan.current,
            fan.target,
            fan.min,
            fan.max,
            if fan.manual { ", manual" } else { "" }
        );
    }
    let temps = smc.temperatures()?;
    println!("{} temperature keys:", temps.len());
    for (key, v) in temps {
        let ty = smc
            .key_info(key)
            .map(|i| i.data_type.to_string())
            .unwrap_or_default();
        println!("  {key} [{ty}] {v:.2}");
    }
    Ok(())
}
