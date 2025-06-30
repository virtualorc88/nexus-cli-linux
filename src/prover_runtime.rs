//! Prover Runtime - Memory Optimized Version
//!
//! Solves 0.7.9 high memory usage issues while maintaining full orchestrator interaction for proper scoring

use crate::environment::Environment;
use std::time::Duration;
use tokio::sync::broadcast;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::Mutex;
use once_cell::sync::Lazy;

// High-performance timestamp cache - avoid duplicate formatting
static LAST_TIMESTAMP_SEC: AtomicU64 = AtomicU64::new(0);
static CACHED_TIMESTAMP: Lazy<Mutex<String>> = Lazy::new(|| {
    Mutex::new(chrono::Local::now().format("%H:%M:%S").to_string())
});

/// High-performance timestamp generation - second-level caching to avoid duplicate formatting
fn get_timestamp_efficient() -> String {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    let last = LAST_TIMESTAMP_SEC.load(Ordering::Relaxed);
    
    if now_secs != last && LAST_TIMESTAMP_SEC.compare_exchange_weak(
        last, now_secs, Ordering::Relaxed, Ordering::Relaxed
    ).is_ok() {
        // Only reformat when seconds change
        let new_timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        *CACHED_TIMESTAMP.lock() = new_timestamp.clone();
        new_timestamp
    } else {
        // Use cached timestamp
        CACHED_TIMESTAMP.lock().clone()
    }
}

/// Memory-optimized authenticated proving loop with task pool - reduces API calls and handles 429 errors
pub async fn run_authenticated_proving_optimized(
    node_id: u64,
    environment: Environment,
    prefix: String,
    _proof_interval: u64,
    mut shutdown: broadcast::Receiver<()>,
    status_callback: Option<Box<dyn Fn(String) + Send + Sync>>,
) -> Result<(), crate::prover::ProverError> {
    use crate::orchestrator_client::OrchestratorClient;
    use crate::prover::get_or_create_prover;
    use crate::task_pool::TaskPool;
    
    let orchestrator_client = OrchestratorClient::new(environment.clone());
    let prover = get_or_create_prover().await?;
    let mut proof_count = 1;
    let mut consecutive_failures = 0;
    
    // Create task pool with capacity similar to 0.8.14 design
    let task_pool = TaskPool::new(100);
    const MIN_TASKS_THRESHOLD: usize = 3;
    let mut last_batch_fetch = std::time::Instant::now() - std::time::Duration::from_secs(10);
    const BATCH_FETCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
    let mut fetch_existing_tasks = true;
    
    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                let msg = "Stopped".to_string();
                if let Some(ref callback) = status_callback {
                    callback(msg);
                } else {
                    println!("{}: Stopped", prefix);
                }
                break;
            }
            _ = async {
                let now = std::time::Instant::now();
                let pool_size = task_pool.len().await;
                

                if fetch_existing_tasks || (task_pool.should_fetch_more(MIN_TASKS_THRESHOLD).await && 
                   now.duration_since(last_batch_fetch) >= BATCH_FETCH_INTERVAL) {
                    
                    let timestamp = get_timestamp_efficient();
                    match orchestrator_client.get_tasks_batch(&node_id.to_string()).await {
                        Ok(tasks) => {
                            if !tasks.is_empty() {
                                let added = task_pool.add_tasks(tasks).await;
                                let new_pool_size = task_pool.len().await;
                                last_batch_fetch = now;
                                fetch_existing_tasks = false;
                                
                                let msg = format!("[{}] 📥 Get {} tasks [tasks:{}]", timestamp, added, new_pool_size);
                                if let Some(ref callback) = status_callback {
                                    callback(msg);
                                } else {
                                    println!("{}: [{}] 📥 Get {} tasks [tasks:{}]", prefix, timestamp, added, new_pool_size);
                                }
                            } else {
                                fetch_existing_tasks = false;
                            }
                        }
                        Err(e) if e.to_string().contains("RATE_LIMITED") => {
                            let msg = format!("[{}] ⚠️ Rate limited - using cache [tasks:{}]", timestamp, pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] ⚠️ Rate limited - using cache [tasks:{}]", prefix, timestamp, pool_size);
                            }
                            fetch_existing_tasks = true;
                            last_batch_fetch = now + std::time::Duration::from_secs(5);
                        }
                        Err(e) => {
                            let _error_string = e.to_string();
                            let _error_msg = _error_string.split(':').last().unwrap_or("error").trim();
                            let msg = format!("[{}] ❌ Fetch task fail [tasks:{}]", timestamp, pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] ❌ Fetch task fail [tasks:{}]", prefix, timestamp, pool_size);
                            }
                            fetch_existing_tasks = false;
                        }
                    }
                }
                
                // Try to get a task from the pool
                if let Some(task) = task_pool.get_next_task().await {
                    let timestamp = get_timestamp_efficient();
                    let current_pool_size = task_pool.len().await;
                    
                    // Process the task directly using the new task-based interface
                    match process_task_with_signing(task.clone(), &orchestrator_client, &prover, &prefix, status_callback.as_ref()).await {
                        Ok(_) => {
                            consecutive_failures = 0;
                            task_pool.mark_task_completed(task.task_id).await;
                            
                            let msg = format!("[{}] ✅ Proof #{} submitted [tasks:{}]", timestamp, proof_count, current_pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] ✅ Proof #{} submitted [tasks:{}]", prefix, timestamp, proof_count, current_pool_size);
                            }
                            proof_count += 1;
                            
                            // Analytics tracking
                            let client_id = format!("{:x}", md5::compute(node_id.to_le_bytes()));
                            crate::analytics::track(
                                "cli_proof_node_v2".to_string(),
                                format!("Completed proof iteration #{}", proof_count),
                                serde_json::json!({
                                    "node_id": node_id,
                                    "proof_count": proof_count,
                                }),
                                false,
                                &environment,
                                client_id,
                            );
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            let error_string = e.to_string();
                            let error_msg = error_string.split(':').last().unwrap_or("error").trim();
                            let msg = format!("[{}] ❌ Proof #{} failed: {} [tasks:{}]", timestamp, proof_count, error_msg, current_pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] ❌ Proof #{} failed: {} [tasks:{}]", prefix, timestamp, proof_count, error_msg, current_pool_size);
                            }
                            
                            // Don't increment proof_count on failure, retry the same number
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            return; // Exit current async block, outer loop continues
                        }
                    }
                } else if !fetch_existing_tasks {
                    let timestamp = get_timestamp_efficient();
                    match orchestrator_client.get_task(&node_id.to_string()).await {
                        Ok(task) => {
                            let current_pool_size = task_pool.len().await;
                            match process_task_with_signing(task.clone(), &orchestrator_client, &prover, &prefix, status_callback.as_ref()).await {
                                Ok(_) => {
                                    consecutive_failures = 0;
                                    task_pool.mark_task_completed(task.task_id).await;
                                    
                                    let msg = format!("[{}] ✅ Proof #{} submitted [tasks:{}]", timestamp, proof_count, current_pool_size);
                                    if let Some(ref callback) = status_callback {
                                        callback(msg);
                                    } else {
                                        println!("{}: [{}] ✅ Proof #{} submitted [tasks:{}]", prefix, timestamp, proof_count, current_pool_size);
                                    }
                                    proof_count += 1;
                                }
                                Err(e) => {
                                    consecutive_failures += 1;
                                    let error_string = e.to_string();
                                    let error_msg = error_string.split(':').last().unwrap_or("error").trim();
                                    let msg = format!("[{}] ❌ Proof #{} failed: {} [tasks:{}]", timestamp, proof_count, error_msg, current_pool_size);
                                    if let Some(ref callback) = status_callback {
                                        callback(msg);
                                    } else {
                                        println!("{}: [{}] ❌ Proof #{} failed: {} [tasks:{}]", prefix, timestamp, proof_count, error_msg, current_pool_size);
                                    }
                                    
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                    return;
                                }
                            }
                        }
                        Err(e) if e.to_string().contains("RATE_LIMITED") => {
                            let current_pool_size = task_pool.len().await;
                            let msg = format!("[{}] ⚠️ Rate limited - retry in 60s [tasks:{}]", timestamp, current_pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] ⚠️ Rate limited - retry in 60s [tasks:{}]", prefix, timestamp, current_pool_size);
                            }
                            fetch_existing_tasks = true;
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            return;
                        }
                        Err(_) => {
                            // No tasks available, enable batch fetching for next cycle
                            let current_pool_size = task_pool.len().await;
                            let msg = format!("[{}] 🔍 No tasks - waiting [tasks:{}]", timestamp, current_pool_size);
                            if let Some(ref callback) = status_callback {
                                callback(msg);
                            } else {
                                println!("{}: [{}] 🔍 No tasks - waiting [tasks:{}]", prefix, timestamp, current_pool_size);
                            }
                            fetch_existing_tasks = true;
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            return;
                        }
                    }
                }
                
                let sleep_duration = if fetch_existing_tasks {
                    Duration::from_secs(3)
                } else {
                    Duration::from_millis(500)
                };
                tokio::time::sleep(sleep_duration).await;
            } => {}
        }
    }
    
    Ok(())
}

/// Memory-optimized anonymous proving loop
#[allow(dead_code)]
pub async fn run_anonymous_proving_optimized(
    environment: Environment,
    prefix: String,
    proof_interval: u64,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), crate::prover::ProverError> {
    use crate::prover::prove_anonymously;
    
    let client_id = format!("{:x}", md5::compute(b"anonymous"));
    let mut proof_count = 1;
    let mut consecutive_failures = 0;
    
    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                println!("{} 🛑 Shutdown signal received", prefix);
                break;
            }
            _ = async {
                let timestamp = get_timestamp_efficient();
                
                match prove_anonymously() {
                    Ok(_) => {
                        consecutive_failures = 0;
                        println!("{} [{}] ✅ Anonymous proof #{} done", prefix, timestamp, proof_count);
                        
                        crate::analytics::track(
                            "cli_proof_anon_v2".to_string(),
                            format!("Completed anon proof iteration #{}", proof_count),
                            serde_json::json!({
                                "node_id": "anonymous",
                                "proof_count": proof_count,
                            }),
                            false,
                            &environment,
                            client_id.clone(),
                        );
                        proof_count += 1;
                        tokio::time::sleep(Duration::from_secs(proof_interval)).await;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        println!("{} [{}] ❌ Anonymous proof #{} failed: {} ({}/∞)", prefix, timestamp, proof_count, e, consecutive_failures);
                        
                        // Infinite retry, wait 5s after failure before continuing (don't increment proof_count)
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        return; // Exit current async block, outer loop continues
                    }
                }
            } => {}
        }
    }
    
    Ok(())
}

/// Process a single task with proof generation and submission - used by task pool system
async fn process_task_with_signing(
    task: crate::task::Task,
    orchestrator_client: &crate::orchestrator_client::OrchestratorClient,
    _prover: &std::sync::Arc<nexus_sdk::stwo::seq::Stwo<nexus_sdk::Local>>,
    prefix: &str,
    status_callback: Option<&Box<dyn Fn(String) + Send + Sync>>,
) -> Result<(), crate::prover::ProverError> {
    use crate::prover::{prove_with_task, ProverError};
    use crate::keys;
    use sha3::{Digest, Keccak256};
    
    // Load signing key
    let signing_key = keys::load_or_generate_signing_key()
        .map_err(|e| ProverError::Orchestrator(format!("Failed to load signing key: {}", e)))?;

    // Generate proof using the task-based proving function
    let proof = prove_with_task(&task)
        .map_err(|e| {
            match e {
                ProverError::MalformedTask(msg) => ProverError::MalformedTask(format!("Task validation failed: {}", msg)),
                ProverError::GuestProgram(msg) => ProverError::GuestProgram(format!("Program execution failed: {}", msg)),
                ProverError::Stwo(msg) => ProverError::Stwo(format!("Prover error: {}", msg)),
                other => other,
            }
        })?;
    
    let timestamp = get_timestamp_efficient();
    let msg = format!("[{}] 💻 Compute completed", timestamp);
    if let Some(ref callback) = status_callback {
        callback(msg);
    } else {
        println!("{}: [{}] 💻 Compute completed", prefix, timestamp);
    }
    
    let proof_hash = format!("{:x}", Keccak256::digest(&proof));

    // Submit proof with signature
    orchestrator_client
        .submit_proof_with_signature(&task.task_id, &proof_hash, proof, signing_key)
        .await
        .map_err(|e| {
            let error_str = e.to_string();
            if error_str.contains("RATE_LIMITED:") {
                ProverError::RateLimited(format!("Proof submission rate limited: {}", error_str))
            } else {
                ProverError::Orchestrator(format!("Failed to submit proof: {}", error_str))
            }
        })?;

    Ok(())
} 