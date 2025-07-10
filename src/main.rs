use actix_web::{
    App, 
    HttpServer, 
    web, 
    HttpResponse, 
    Responder,
    error::ErrorInternalServerError
};
use actix_cors::Cors;
use dotenv::dotenv;
use tracing;
use std::str::FromStr;
use std::fs;
use serde_json;
use delta_executor_sdk::{
    self,
    base::{
        core::Shard,
        crypto::{
            HashDigest,
            Ed25519PubKey,
            Ed25519PrivKey,
        }, 
        verifiable::VerifiableType,
    },
    execution::FullDebitExecutor,
    proving, storage::options::DbOptions
};
// Import the renamed types from the new locations
use std::{
    env,
    path::PathBuf,
    sync::Mutex,
};

// Types to store state
type Buffer = Mutex<Vec<VerifiableType>>;

// Structure to track execution batches for submission
#[derive(Default)]
struct BatchState {
    transaction_count: usize,
}

type BatchTracker = Mutex<BatchState>;


// Define the specific Runtime type we're using
// 
type Runtime = delta_executor_sdk::Runtime<FullDebitExecutor, proving::mock::Client>;

// Function to read keypair from JSON file
fn read_keypair(path: &PathBuf) -> Result<Ed25519PrivKey, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    // Parse as JSON string
    let key_str: String = serde_json::from_str(&content)?;
    let privkey = Ed25519PrivKey::from_str(&key_str)?;
    Ok(privkey)
}

fn read_key(key_path: &PathBuf) -> Result<(Ed25519PrivKey, Ed25519PubKey), Box<dyn std::error::Error>> {
    let my_key: Ed25519PrivKey = read_keypair(key_path)?;
    let pubkey = my_key.pub_key();
    tracing::info!(
        "Using key from {} with pubkey: {}",
        key_path.display(),
        pubkey
    );
    Ok((my_key, pubkey))
}


// Health check endpoint
async fn health_check() -> HttpResponse {
    let health = serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "environment": env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()),
    });
    HttpResponse::Ok().json(health)
}

// Runtime info endpoint  
async fn runtime_info() -> HttpResponse {
    let shard = env::var("SHARD_NUMBER").unwrap_or_else(|_| "1".to_string());
    let info = serde_json::json!({
        "status": "runtime_active",
        "shard": shard,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    HttpResponse::Ok().json(info)
}

async fn get_vault(key: web::Path<Ed25519PubKey>, runtime: web::Data<Runtime>) -> HttpResponse {
    tracing::info!("Received request for vault: {}", key);

    match runtime.get_vault(&key.into_inner()) {
        Ok(Some(vault)) => HttpResponse::Ok().json(vault),
        Ok(None) => HttpResponse::NotFound().body("Vault not found for the provided public key"),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

async fn post_signed_verifiables(
    request: web::Json<Vec<VerifiableType>>,
    buffer: web::Data<Buffer>,
) -> actix_web::Result<HttpResponse> {
    buffer
        .lock()
        .map_err(|e| ErrorInternalServerError(e.to_string()))?
        .extend(request.into_inner());

    Ok(HttpResponse::Ok().finish())
}


async fn post_execute(
    request: web::Json<Vec<VerifiableType>>,
    runtime: web::Data<Runtime>,
    batch_tracker: web::Data<BatchTracker>,
    batch_size: web::Data<usize>,
) -> actix_web::Result<impl Responder> {
    // Get the verifiables from the request
    let verifiables = request.into_inner();
    let verifiable_count = verifiables.len();
    tracing::info!("Processing {} verifiables", verifiable_count);
    
    // Execute the verifiables
    runtime.execute(verifiables).await
        .map_err(|e| {
            tracing::error!("Error executing verifiables: {:?}", e);
            ErrorInternalServerError(e.to_string())
        })?;
    
    tracing::info!("Successfully executed verifiables");
    
    // Update batch tracker and check if we need to submit
    let mut batch_state = batch_tracker
        .lock()
        .map_err(|e| ErrorInternalServerError(e.to_string()))?;
    
    batch_state.transaction_count += verifiable_count;
    let current_count = batch_state.transaction_count;
    
    // Check if we've reached the batch size
    if current_count >= **batch_size {
        tracing::info!("Batch size reached ({}/{}), submitting to base layer", 
            current_count, **batch_size);
        
        // Reset counter
        batch_state.transaction_count = 0;
        
        // Release lock before submitting
        drop(batch_state);
        
        // Submit accumulated diffs to base layer
        match runtime.submit().await {
            Ok(Some(sdl_hash)) => {
                tracing::info!("Successfully submitted batch, SDL hash: {:?}", sdl_hash);
                
                // Auto-prove configurable via env var
                if env::var("AUTO_PROVE").unwrap_or_else(|_| "false".to_string()) == "true" {
                    match runtime.prove(sdl_hash).await {
                        Ok(_) => tracing::info!("Started proving for SDL: {:?}", sdl_hash),
                        Err(e) => tracing::error!("Failed to start proving: {:?}", e),
                    }
                }
                
                Ok(HttpResponse::Ok().json(serde_json::json!({
                    "status": "executed_and_submitted",
                    "sdl_hash": sdl_hash
                })))
            },
            Ok(None) => {
                tracing::warn!("Submit returned None - no diffs to submit");
                Ok(HttpResponse::Ok().json(serde_json::json!({
                    "status": "executed_only",
                    "message": "No diffs to submit"
                })))
            },
            Err(e) => {
                tracing::error!("Failed to submit batch: {:?}", e);
                // Execution succeeded but submission failed - still return success
                // but indicate the submission failure
                Ok(HttpResponse::Ok().json(serde_json::json!({
                    "status": "executed_only",
                    "error": format!("Submit failed: {}", e)
                })))
            }
        }
    } else {
        tracing::info!("Batch not full yet ({}/{}), execution only", current_count, **batch_size);
        Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "executed_only",
            "batch_progress": format!("{}/{}", current_count, **batch_size)
        })))
    }
}

async fn post_submit_proof(
    request: web::Json<HashDigest>,
    runtime: web::Data<Runtime>,
) -> actix_web::Result<impl Responder> {
    runtime
        .submit_proof(request.into_inner())
        .await
        .map_err(ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().finish())
}

// Manual submit endpoint to force submission before batch is full
async fn post_submit(
    runtime: web::Data<Runtime>,
    batch_tracker: web::Data<BatchTracker>,
) -> actix_web::Result<impl Responder> {
    // Reset the batch counter
    let mut batch_state = batch_tracker
        .lock()
        .map_err(|e| ErrorInternalServerError(e.to_string()))?;
    
    let transaction_count = batch_state.transaction_count;
    batch_state.transaction_count = 0;
    drop(batch_state);
    
    // Submit accumulated diffs
    match runtime.submit().await {
        Ok(Some(sdl_hash)) => {
            tracing::info!("Manual submit successful, SDL hash: {:?}", sdl_hash);
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "status": "submitted",
                "sdl_hash": sdl_hash,
                "transactions_submitted": transaction_count
            })))
        },
        Ok(None) => {
            tracing::info!("No diffs to submit");
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "status": "no_diffs",
                "message": "No pending diffs to submit"
            })))
        },
        Err(e) => {
            tracing::error!("Submit failed: {:?}", e);
            Err(ErrorInternalServerError(e.to_string()))
        }
    }
}

// Execute, submit, and prove in one call (original behavior)
async fn execute_submit_prove(
    request: web::Json<Vec<VerifiableType>>,
    runtime: web::Data<Runtime>,
) -> actix_web::Result<impl Responder> {
    let messages = request.into_inner();
    let sdl = runtime
        .execute_submit_prove(messages)
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(web::Json(sdl))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenv().ok();
    
    // Install a global subscriber, configured based on the RUST_LOG env var
    tracing_subscriber::fmt::init();
    
    // Get environment configuration
    let environment = env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string());
    tracing::info!("Running in {} mode", environment);
    
    // Batch size for submission (not proof)
    let batch_size = env::var("SUBMIT_BATCH_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10);
    
    tracing::info!("Submit batch size: {} transactions", batch_size);
    
    
    let shard = env::var("SHARD_NUMBER")
        .ok()
        .and_then(|s| s.parse::<Shard>().ok())
        .unwrap_or(1);
    
    // Read keypair from environment variable, file, or generate new one
    let (keypair, _pubkey) = if let Ok(privkey_str) = env::var("EXECUTOR_PRIVATE_KEY") {
        // Read from environment variable
        tracing::info!("Reading keypair from EXECUTOR_PRIVATE_KEY environment variable");
        let privkey = Ed25519PrivKey::from_str(&privkey_str)
            .map_err(|e| format!("Invalid private key in EXECUTOR_PRIVATE_KEY: {}", e))?;
        let pubkey = privkey.pub_key();
        tracing::info!("Using executor with public key: {}", pubkey);
        (privkey, pubkey)
    } else {
        // Fall back to file-based approach
        let keypair_path = env::var("KEYPAIR_PATH")
            .unwrap_or_else(|_| "keypair.json".to_string());
        
        if PathBuf::from(&keypair_path).exists() {
            tracing::info!("Reading keypair from {}", keypair_path);
            read_key(&PathBuf::from(&keypair_path))?
        } else {
            tracing::warn!("No EXECUTOR_PRIVATE_KEY env var and keypair file {} not found, generating new keypair", keypair_path);
            let new_keypair = Ed25519PrivKey::generate();
            let new_pubkey = new_keypair.pub_key();
            
            // Save the generated keypair to file
            fs::write(&keypair_path, new_keypair.to_string())?;
            tracing::info!("Saved new keypair to {}", keypair_path);
            tracing::info!("Public key: {}", new_pubkey);
            
            (new_keypair, new_pubkey)
        }
    };

    // Get configuration from environment variables with fallbacks
    let host = env::var("SERVER_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = env::var("SERVER_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8081);

     // Read DB files path from env or use some tempdir (default)
     let db_options = env::var("DB_PATH")
        .map(|p| DbOptions::default().with_db_prefix_path(p.into()))
        .unwrap_or_default();

    // Create the runtime with environment-based configuration
    let runtime: web::Data<Runtime> = if environment == "production" {
        let base_rpc_url = env::var("BASE_RPC_URL")
            .expect("BASE_RPC_URL must be set in production");
        tracing::info!("Production mode: Connecting to base layer at {}", base_rpc_url);
        tracing::info!("Building runtime for shard {} with pubkey {}", shard, _pubkey);
        
        let runtime = delta_executor_sdk::Runtime::builder(shard, keypair)
            .with_rocksdb(db_options)
            .with_rpc(&base_rpc_url)
            .with_seed_keys(vec![_pubkey])
            .build()
            .await?;
        
        tracing::info!("Runtime successfully created with RPC connection");
        
        // Try to get our own vault to verify connection
        match runtime.get_vault(&_pubkey) {
            Ok(Some(_)) => tracing::info!("Found vault for executor pubkey: {}", _pubkey),
            Ok(None) => tracing::warn!("No vault found for executor pubkey: {}", _pubkey),
            Err(e) => tracing::error!("Error checking executor vault: {:?}", e),
        }
        
        web::Data::new(runtime)
    } else {
        tracing::info!("Development mode: Running without RPC connection");
        tracing::info!("Building runtime for shard {} with pubkey {}", shard, _pubkey);
        
        web::Data::new(
            delta_executor_sdk::Runtime::builder(shard, keypair)
                .build()
                .await?,
        )
    };


    // Buffer to store incoming debits until they are executed
    let buffer = web::Data::new(Buffer::new(vec![]));
    
    // Batch tracker for submission
    let batch_tracker = web::Data::new(BatchTracker::default());
    let batch_size_data = web::Data::new(batch_size);

    tracing::info!("Starting server on http://{}:{}", host, port);
    
    // start web server
    let rt = runtime.clone();
    let server = HttpServer::new(move || {
        // Use permissive CORS for all environments (for debugging)
        let cors = Cors::permissive();
        
        App::new()
            .wrap(cors)
            .app_data(buffer.clone())
            .app_data(rt.clone())
            .app_data(batch_tracker.clone())
            .app_data(batch_size_data.clone())
            .route("/health", web::get().to(health_check))
            .route("/runtime-info", web::get().to(runtime_info))
            .route("/vaults/{pubkey}", web::get().to(get_vault))
            .route("/verifiables", web::post().to(post_signed_verifiables))
            .route("/execute", web::post().to(post_execute))
            .route("/submit", web::post().to(post_submit))
            .route("/submit-proof", web::post().to(post_submit_proof))
            .route("/execute-submit-prove", web::post().to(execute_submit_prove))
    })
    .bind((host, port))?
    .run();

    // Create a signal handler for graceful shutdown
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    
    // Handle Ctrl+C
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl+c");
        tracing::info!("Received shutdown signal");
        let _ = tx.send(());
    };
    
    // Run the server in the background
    let server_handle = tokio::spawn(server);
    
    // Run the runtime with a shutdown handler
    let runtime_handle = tokio::spawn(async move {
        tokio::select! {
            _ = runtime.run() => tracing::info!("Runtime completed"),
            _ = rx => tracing::info!("Runtime shutting down")
        }
    });
    
    // Wait for Ctrl+C
    tokio::select! {
        _ = ctrl_c => {},
        _ = server_handle => tracing::info!("Server stopped unexpectedly"),
        _ = runtime_handle => tracing::info!("Runtime stopped unexpectedly")
    }
    
    tracing::info!("Shutting down gracefully");
    Ok(())
}