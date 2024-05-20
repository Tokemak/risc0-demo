// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use alloy_sol_types::SolValue;
use anyhow::{Context, Result};
use clap::Parser;
use methods::TOKEN_STATS_ELF;
use risc0_steel::{
    config::ETH_MAINNET_CHAIN_SPEC,
    ethereum::{EthBlockHeader, EthViewCallEnv},
    host::{
        provider::{CachedProvider, EthersProvider, Provider},
        EthersClient,
    },
    ViewCall, ViewCallInput,
};
use risc0_zkvm::{default_executor, ExecutorEnv};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokemak::{BLOCKS_TO_QUERY, BLOCK_GRANULARITY, cbETHInterface, CBETH_ADDRESS, LstDexStats};
use tracing_subscriber::EnvFilter;

// Simple program to show the use of Ethereum contract data inside the guest.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// URL of the RPC endpoint
    #[arg(short, long, env = "RPC_URL")]
    rpc_url: String,
    /// Directory to cache responses
    #[arg(short, long, env = "CACHE_DIR")]
    cache_dir: String,
    /// Start Block Number
    #[arg(short, long, env = "END_BLOCK_NUMBER")]
    end_block_number: Option<String>,
}

fn main() -> Result<()> {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();
    // parse the command line arguments
    let args = Args::parse();

    // Create a view call environment from an RPC endpoint and a block number. If no block number is
    // provided, the latest block is used. The `with_chain_spec` method is used to specify the
    // chain configuration.
    let cache_dir = PathBuf::from(args.cache_dir);
    let client = EthersClient::new_client(&args.rpc_url, 3, 500)?;
    let provider = EthersProvider::new(client);

    let head_block_num = if let Some(end_block) = &args.end_block_number {
        end_block.parse::<u64>().unwrap()
    } else {
        provider.get_block_number()?
    };

    let cache_provider = CachedProvider::new(cache_dir.clone(), provider).unwrap();

    // Take a block x behind head, to check hash linking to commitment
    let query_block_num = head_block_num - BLOCKS_TO_QUERY;

    // used for logging time for specific operations
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

    // TODO: parallelize
    // headers used for historical header validation
    let headers_from_query: Vec<_> = (query_block_num..=head_block_num)
        .into_iter()
        .map(|block_num| {
            cache_provider
                .get_block_header(block_num)
                .with_context(|| format!("could not retrieve block {block_num}"))?
                .with_context(|| format!("block at height {block_num} not found"))
        })
        .collect::<Result<_>>()?;
    let current_time = log_time_delta("get_headers", current_time);

    // manually drop the cached provider to ensure it writes its data
    drop(cache_provider);

    // TODO: parallelize
    let mut inputs: Vec<ViewCallInput<EthBlockHeader>> = Vec::new();
    for block_num in (query_block_num..=head_block_num).step_by(BLOCK_GRANULARITY as usize) {
        let c = EthersClient::new_client(&args.rpc_url, 3, 500)?;
        let p = EthersProvider::new(c);
        let cp = CachedProvider::new(cache_dir.clone(), p)?;

        let mut env =
            EthViewCallEnv::from_provider(cp, block_num)?.with_chain_spec(&ETH_MAINNET_CHAIN_SPEC);

        env.preflight(ViewCall::new(cbETHInterface::exchangeRateCall {}, CBETH_ADDRESS))?._0;

        let input = env.into_zkvm_input()?;
        inputs.push(input);
    }
    let current_time = log_time_delta("preflights", current_time);

    println!("Running the guest with the constructed input:");
    let session_info = {
        let env = ExecutorEnv::builder()
            .write(&inputs)?
            .write(&headers_from_query)?
            .build()
            .context("Failed to build exec env")?;
        let exec = default_executor();
        exec.execute(env, TOKEN_STATS_ELF).context("failed to run executor")?
    };
    let current_time = log_time_delta("executor", current_time);

    let stats = LstDexStats::abi_decode(&session_info.journal.bytes, true)?;
    println!("{}", stats);
    log_time_delta("end", current_time);
    Ok(())
}

fn log_time_delta(name: &str, start: Duration) -> Duration {
    let now_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let delta = (now_time - start).as_secs();
    println!("{} took {} seconds", name, delta);

    now_time
}
