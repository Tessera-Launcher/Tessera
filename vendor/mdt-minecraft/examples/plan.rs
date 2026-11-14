//! Write one download list, without downloading anything.
//!
//! ```sh
//! cargo run --example plan -- <root> <mc-version> [loader-version]
//! cargo run --example plan -- /tmp/mdt 26.3 0.20.1
//! MDT_MIRROR=bmclapi cargo run --example plan -- /tmp/mdt 26.3 0.20.1
//! ```
//!
//! Prints the list path and a few numbers; the file itself is the aria2
//! `--input-file` body.

use std::process::ExitCode;

use mdt::{MdtClient, Mirror, ModLoader};

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(version)) = (args.next(), args.next()) else {
        eprintln!("usage: plan <root> <mc-version> [loader-version]");
        return ExitCode::FAILURE;
    };
    let loader_version = args.next();

    let mirror = match std::env::var("MDT_MIRROR").as_deref() {
        Ok("bmclapi") => Mirror::BMCLAPI,
        Ok("mojang") | Err(_) => Mirror::Mojang,
        Ok(other) => Mirror::Custom(other.to_owned()),
    };

    let client = match MdtClient::builder().mirror(mirror).root_dir(&root).build() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("client: {error}");
            return ExitCode::FAILURE;
        }
    };

    let plan = match loader_version {
        Some(loader_version) => {
            client
                .plan_loader(&version, ModLoader::Fabric, &loader_version)
                .await
        }
        None => client.plan_vanilla(&version).await,
    };

    match plan {
        Ok(plan) => {
            println!("id:      {}", plan.id);
            println!("files:   {}", plan.entries.len());
            println!("hashed:  {}", plan.hashed());
            println!("bytes:   {}", plan.total_bytes());
            println!("list:    {}", plan.path(client.root_dir()).display());
            if std::env::var("MDT_VERIFY").is_ok() {
                let started = std::time::Instant::now();
                let failed = plan.verify(client.root_dir()).await;
                println!(
                    "verify:  {} 个文件不符（{} 个条目，{:.1}s）",
                    failed.len(),
                    plan.entries.len(),
                    started.elapsed().as_secs_f64(),
                );
                for entry in failed.iter().take(5) {
                    println!("  缺/坏: {}", entry.path.display());
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("plan: {error}");
            ExitCode::FAILURE
        }
    }
}
