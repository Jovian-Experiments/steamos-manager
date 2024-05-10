/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use clap::Parser;

use steamos_manager::daemon;

#[derive(Parser)]
struct Args {
    /// Run the root manager daemon
    #[arg(short, long)]
    root: bool,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let args = Args::parse();
    if args.root {
        daemon::root().await
    } else {
        daemon::user().await
    }
}
