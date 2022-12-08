mod commands;
mod db;
mod profiler;

use std::{
    cell::RefCell,
    cmp::Ordering,
    ops::{Mul, MulAssign, Sub},
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        mpsc, Arc,
    },
    time::Duration,
};

use commands::*;
use feth::{one_eth_key, parse_call_json, parse_deploy_json, parse_query_json, utils::*, KeyPair, TestClient};
use log::{debug, error, info};
use rayon::prelude::*;
use web3::types::{Address, Block, BlockId, BlockNumber, TransactionId, H256, U256, U64};

fn eth_transaction(network: &str, timeout: Option<u64>, hash: H256) {
    let network = real_network(network);
    // use first endpoint to fund accounts
    let client = TestClient::setup(network[0].clone(), timeout);
    let tx = client.transaction(TransactionId::from(hash));
    log::info!("{:?}", tx);
}

fn eth_account(network: &str, timeout: Option<u64>, account: Address) {
    let network = real_network(network);
    // use first endpoint to fund accounts
    let client = TestClient::setup(network[0].clone(), timeout);
    let balance = client.balance(account, None);
    let nonce = client.nonce(account, None);
    log::info!("{:?}: {} {:?}", account, balance, nonce);
}
fn eth_contract(network: &str, timeout: Option<u64>, optype: &ContractOP, config: &PathBuf) -> anyhow::Result<()> {
    let network = real_network(network);
    let client = TestClient::setup(network[0].clone(), timeout);
    match optype {
        ContractOP::Deploy => {
            let deploy_json = parse_deploy_json(config)?;
            client.contract_deploy(deploy_json)?;
        }
        ContractOP::Call => {
            let call_json = parse_call_json(config)?;
            client.contract_call(call_json)?;
        }
        ContractOP::Query => {
            let query_json = parse_query_json(config)?;
            client.contract_query(query_json)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct BlockInfo {
    number: u64,
    timestamp: U256,
    count: usize,
    block_time: u64,
}

fn para_eth_blocks(client: TestClient, start: u64, end: u64) {
    let client = Arc::new(client);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
    let (tx, rx) = mpsc::channel();
    (start..end).for_each(|n| {
        let tx = tx.clone();
        let client = client.clone();
        pool.install(move || {
            let id = BlockId::Number(BlockNumber::Number(U64::from(n)));
            let b = client.block_with_tx_hashes(id).map(|b| BlockInfo {
                number: b.number.unwrap().as_u64(),
                timestamp: b.timestamp,
                count: b.transactions.len(),
                block_time: 0u64,
            });
            tx.send((n, b)).unwrap();
        })
    });
    let mut blocks = vec![None; (end - start) as usize];
    for _ in start..end {
        let j = rx.recv().unwrap();
        *blocks.get_mut((j.0 - start) as usize).unwrap() = j.1
    }
    blocks.iter().for_each(|b| {
        if let Some(b) = b {
            info!("{},{},{},{}", b.number, b.timestamp, b.count, b.block_time);
        } else {
            info!("None");
        }
    })
}

fn eth_blocks(network: &str, timeout: Option<u64>, start: Option<u64>, count: Option<i64>) {
    let network = real_network(network);
    // use first endpoint to fund accounts
    let client = TestClient::setup(network[0].clone(), timeout);
    if let Some(start) = start {
        let range = count
            .map(|c| match c.cmp(&0i64) {
                Ordering::Equal => start..start + 1,
                Ordering::Less => {
                    let n = c.unsigned_abs() as u64;
                    if start > n {
                        start - n..start + 1
                    } else {
                        0..start + 1
                    }
                }
                Ordering::Greater => start..start + c.unsigned_abs() as u64 + 1,
            })
            .unwrap_or_else(|| match client.block_number() {
                Some(end) if start > end.as_u64() => {
                    panic!(
                        "start block height is bigger than latest height({}>{})",
                        start,
                        end.as_u64()
                    );
                }
                Some(end) => start..end.as_u64() + 1,
                None => panic!("Failed to obtain block height"),
            });
        let _last_block: RefCell<Option<(u64, Block<H256>)>> = RefCell::new(if range.start == 0 {
            None
        } else {
            let id = BlockId::Number(BlockNumber::Number(U64::from(range.start - 1)));
            Some((range.start - 1, client.block_with_tx_hashes(id).unwrap()))
        });
        para_eth_blocks(client, range.start, range.end);
        //range
        //    .map(|number| {
        //        let id = BlockId::Number(BlockNumber::Number(U64::from(number)));
        //        client.block_with_tx_hashes(id).map(|block| {
        //            let block_time = match &*last_block.borrow() {
        //                Some(last) if last.0 + 1 == number => (block.timestamp - last.1.timestamp).as_u64(),
        //                _ => 0u64,
        //            };
        //            let count = block.transactions.len();
        //            let timestamp = block.timestamp;
        //            *last_block.borrow_mut() = Some((number, block));
        //            (number, timestamp, count, block_time)
        //        })
        //    })
        //    .for_each(|block| {
        //        let msg = if let Some(block) = block {
        //            format!("{},{:?},{},{}", block.0, block.1, block.2, block.3)
        //        } else {
        //            "None".to_string()
        //        };
        //        log::info!("{}", msg);
        //    });
    } else if let Some(b) = client.current_block() {
        let block_time = match b.number {
            Some(n) if n > U64::zero() => {
                if let Some(last) = client.block_with_tx_hashes(BlockId::Number(BlockNumber::Number(n.sub(1)))) {
                    Some(b.timestamp - last.timestamp)
                } else {
                    None
                }
            }
            _ => None,
        };
        log::info!(
            "{},{:?},{},{}",
            b.number.unwrap_or_default(),
            b.timestamp,
            b.transactions.len(),
            block_time.unwrap_or_default(),
        );
    } else {
        error!("Cannot obtain current block");
    }
}

fn fund_accounts(
    network: &str,
    timeout: Option<u64>,
    block_time: u64,
    count: u64,
    am: u64,
    load: bool,
    redeposit: bool,
) {
    let mut amount = web3::types::U256::exp10(17); // 0.1 eth
    amount.mul_assign(am);

    let network = real_network(network);
    // use first endpoint to fund accounts
    let client = TestClient::setup(network[0].clone(), timeout);
    let balance = client.balance(client.root_addr, None);
    info!("Balance of {:?}: {}", client.root_addr, balance);

    let mut source_keys = if load {
        let keys: Vec<_> = serde_json::from_str(std::fs::read_to_string("source_keys.001").unwrap().as_str()).unwrap();
        keys
    } else {
        // check if the key file exists
        debug!("generating new source keys");
        if std::fs::File::open("source_keys.001").is_ok() {
            panic!("file \"source_keys.001\" already exists");
        }
        if amount.mul(count + 1) >= balance {
            panic!("Too large source account number, maximum {}", balance / amount);
        }
        let source_keys = (0..count).map(|_| one_eth_key()).collect::<Vec<_>>();
        let data = serde_json::to_string(&source_keys).unwrap();
        std::fs::write("source_keys.001", &data).unwrap();

        source_keys
    };

    // add more source keys and save them to file
    if count as usize > source_keys.len() {
        source_keys.resize_with(count as usize, one_eth_key);

        std::fs::rename("source_keys.001", ".source_keys.001.bak").unwrap();
        let data = serde_json::to_string(&source_keys).unwrap();
        std::fs::write("source_keys.001", &data).unwrap();
    }else if (count as usize) < source_keys.len()
    {
        source_keys.truncate(count as usize);
        std::fs::rename("source_keys.001", ".source_keys.001.bak").unwrap();
        let data = serde_json::to_string(&source_keys).unwrap();
        std::fs::write("source_keys.001", &data).unwrap();
    }


    let total = source_keys.len();
    let source_accounts = source_keys
        .into_iter()
        .enumerate()
        .filter_map(|(idx, key)| {
            let from = Address::from_str(key.address.as_str()).unwrap();
            let account = if redeposit {
                let balance = client.balance(from, None);
                if balance < amount {
                    Some((from, amount))
                } else {
                    None
                }
            } else {
                Some((from, amount))
            };
            if let Some(a) = account.as_ref() {
                log::info!("{}/{} {:?}", idx + 1, total, a);
            }
            account
        })
        .collect::<Vec<_>>();
    // 1000 eth
    let _metrics = client
        .distribution(1, None, &source_accounts, &Some(block_time), true, true)
        .unwrap();
    // save metrics to file
    //let data = serde_json::to_string(&metrics).unwrap();
    //std::fs::write("metrics.001", &data).unwrap();
}


fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse_args();
    debug!("{:?}", cli);
    info!("logical cpus {}, physical cpus {}", log_cpus(), phy_cpus());

    match &cli.command {
        Some(Commands::Fund {
            network,
            timeout,
            block_time,
            count,
            amount,
            load,
            redeposit,
        }) => {
            fund_accounts(
                network.get_url().as_str(),
                *timeout,
                *block_time,
                *count,
                *amount,
                *load,
                *redeposit,
            );
            Ok(())
        }
        Some(Commands::Info {
            network,
            timeout,
            account,
        }) => {
            eth_account(network.get_url().as_str(), *timeout, *account);
            Ok(())
        }
        Some(Commands::Transaction { network, timeout, hash }) => {
            eth_transaction(network.get_url().as_str(), *timeout, *hash);
            Ok(())
        }
        Some(Commands::Block {
            network,
            timeout,
            start,
            count,
        }) => {
            eth_blocks(network.get_url().as_str(), *timeout, *start, *count);
            Ok(())
        }
        Some(Commands::Etl {
            abcid,
            tendermint,
            redis,
            load,
        }) => {
            let _ = Cli::etl_cmd(abcid, tendermint, redis.as_str(), *load);
            Ok(())
        }
        Some(Commands::Profiler { network, enable }) => {
            let _ = Cli::profiler(network.as_str(), *enable);
            Ok(())
        }
        Some(Commands::Contract {
            network,
            optype,
            config,
            timeout,
        }) => {
            let rpc_url = network.get_url();
            eth_contract(&rpc_url, *timeout, optype, config)?;
            Ok(())
        }
        Some(Commands::Test {
            network,
            mode: _,
            delay,
            max_threads,
            count,
            source,
            block_time,
            timeout,
            need_retry,
            check_balance,
        }) => {
            let max_par = *max_threads;
            let source_file = source;
            let _block_time = Some(*block_time);
            let timeout = Some(*timeout);
            let count = *count;
            let _need_retry = *need_retry;

            let source_keys: Vec<KeyPair> =
                serde_json::from_str(std::fs::read_to_string(source_file).unwrap().as_str()).unwrap();
            let target_amount = web3::types::U256::exp10(16); // 0.01 eth

            check_parallel_args(max_par);

            let max_pool_size = calc_pool_size(source_keys.len(), max_par as usize);
            rayon::ThreadPoolBuilder::new()
                .num_threads(max_pool_size)
                .build_global()
                .unwrap();
            info!("thread pool size {}", max_pool_size);

            let url = network.get_url();
            let client = Arc::new(TestClient::setup(Some(url), timeout));

            let chain_id = client.chain_id().unwrap().as_u64();
            let gas_price = client.gas_price().unwrap();
            info!("chain_id:     {}", chain_id);
            info!("gas_price:    {}", gas_price);
            info!("block_number: {}", client.block_number().unwrap());
            info!("frc20 code:   {:?}", client.frc20_code().unwrap());

            info!("preparing test data...");
            let source_keys = source_keys
                .par_iter()
                .filter_map(|kp| {
                    let (secret, address) = (
                        secp256k1::SecretKey::from_str(kp.private.as_str()).unwrap(),
                        Address::from_str(kp.address.as_str()).unwrap(),
                    );
                    let balance = if *check_balance {
                        client.balance(address, None)
                    } else {
                        U256::MAX
                    };
                    if balance > target_amount.mul(count) {
                        let target = (0..count)
                            .map(|_| {
                                (
                                    Address::from_str(one_eth_key().address.as_str()).unwrap(),
                                    target_amount,
                                )
                            })
                            .collect::<Vec<_>>();
                        debug!("account {:?} added to source pool", address);
                        Some((secret, address, target))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            if count == 0 || source_keys.is_empty() {
                error!("Not enough sufficient source accounts or target accounts, skipped.");
                return Ok(());
            }

            let total_succeed = AtomicU64::new(0);
            let concurrences = if source_keys.len() > max_pool_size {
                max_pool_size
            } else {
                source_keys.len()
            };

            // one-thread per source key
            info!("starting tests...");
            let start_height = client.block_number().unwrap();
            let mut last_height = start_height;
            let total = source_keys.len() * count as usize;
            let now = std::time::Instant::now();
            for r in 0..count {
                loop {
                    let current = client.block_number().unwrap();
                    if current > last_height {
                        last_height = current;
                        break;
                    } else {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                let now = std::time::Instant::now();
                source_keys.par_iter().for_each(|(source, address, targets)| {
                    let target = targets.get(r as usize).unwrap();
                    if let Some(nonce) = client.pending_nonce(*address) {
                        if client
                            .distribution_simple(source, target, Some(chain_id), Some(gas_price), Some(nonce))
                            .is_ok()
                        {
                            total_succeed.fetch_add(1, Relaxed);
                        }
                    }
                });
                let elapsed = now.elapsed().as_secs();
                info!("round {}/{} time {}", r + 1, count, elapsed);
                std::thread::sleep(Duration::from_secs(*delay));
            }

            let elapsed = now.elapsed().as_secs();
            let end_height = client.block_number().unwrap();

            let avg = total as f64 / elapsed as f64;
            info!(
                "Test result summary: total,{:?}/{},concurrency,{},TPS,{:.3},seconds,{},height,{},{}",
                total_succeed, total, concurrences, avg, elapsed, start_height, end_height,
            );
            Ok(())
        }
        None => Ok(()),
    }
}
