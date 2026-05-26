# Rust Design Patterns Guide

This guide provides practical Rust design patterns, idioms, and best practices for building robust, maintainable code. Based on the [Rust Unofficial Patterns](https://rust-unofficial.github.io/patterns/) collection.

## Table of Contents

1. [Idioms](#idioms) - Rust-specific coding conventions
2. [Design Patterns](#design-patterns) - Reusable solutions to common problems
3. [Anti-Patterns](#anti-patterns) - Common mistakes to avoid
4. [Error Handling](#error-handling) - Robust error management
5. [API Design](#api-design) - Building good APIs
6. [Concurrency](#concurrency) - Async and parallel programming
7. [Memory Management](#memory-management) - Ownership and borrowing
8. [Code Organization](#code-organization) - Project structure

---

## Idioms

### 1. Use `?` for Error Propagation

**Pattern**: Use the `?` operator instead of manual error handling.

```rust
// ✅ Good - Using ?
fn read_file_contents(path: &str) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

// ❌ Avoid - Manual error handling
fn read_file_contents_manual(path: &str) -> Result<String, std::io::Error> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => return Err(e),
    };
    let mut contents = String::new();
    match file.read_to_string(&mut contents) {
        Ok(_) => Ok(contents),
        Err(e) => Err(e),
    }
}
```

### 2. Prefer `if let` for Single Pattern Matching

**Pattern**: Use `if let` instead of `match` for single pattern cases.

```rust
// ✅ Good - Using if let
if let Some(value) = optional_value {
    println!("Got value: {}", value);
}

// ❌ Verbose - Using match for single pattern
match optional_value {
    Some(value) => println!("Got value: {}", value),
    None => {}
}
```

### 3. Use Type Aliases for Complex Types

**Pattern**: Create type aliases for complex or frequently used types.

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ✅ Good - Clear type aliases
type UserId = u64;
type UserDatabase = Arc<RwLock<HashMap<UserId, User>>>;
type Result<T> = std::result::Result<T, MyError>;

// Usage becomes much clearer
fn get_user(db: &UserDatabase, id: UserId) -> Result<Option<User>> {
    let users = db.read().unwrap();
    Ok(users.get(&id).cloned())
}
```

### 4. Prefer `Default` over Manual Initialization

**Pattern**: Implement `Default` trait instead of custom `new()` functions.

```rust
#[derive(Default)]
struct Config {
    host: String,
    port: u16,
    timeout_seconds: u64,
}

impl Config {
    // ✅ Good - Use Default and builder pattern
    fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }
    
    fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

// Usage
let config = Config::default()
    .with_host("localhost")
    .with_port(8080);
```

---

## Design Patterns

### 1. Builder Pattern

**Use Case**: Complex object construction with optional parameters.

```rust
#[derive(Debug)]
pub struct HttpClient {
    base_url: String,
    timeout: Duration,
    retries: u32,
    headers: HashMap<String, String>,
}

pub struct HttpClientBuilder {
    base_url: String,
    timeout: Duration,
    retries: u32,
    headers: HashMap<String, String>,
}

impl HttpClientBuilder {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            timeout: Duration::from_secs(30),
            retries: 3,
            headers: HashMap::new(),
        }
    }
    
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }
    
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
    
    pub fn build(self) -> HttpClient {
        HttpClient {
            base_url: self.base_url,
            timeout: self.timeout,
            retries: self.retries,
            headers: self.headers,
        }
    }
}

// Usage
let client = HttpClientBuilder::new("https://api.example.com")
    .timeout(Duration::from_secs(10))
    .retries(5)
    .header("Authorization", "Bearer token")
    .build();
```

### 2. Newtype Pattern

**Use Case**: Type safety and API design.

```rust
// ✅ Good - Strong typing
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct UserId(u64);

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ProductId(u64);

impl UserId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
    
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

// This prevents mixing up different types of IDs
fn get_user_orders(user_id: UserId, product_id: ProductId) -> Vec<Order> {
    // Implementation
}

// ❌ Bad - Easy to mix up parameters
fn get_user_orders_bad(user_id: u64, product_id: u64) -> Vec<Order> {
    // Can accidentally pass product_id as user_id
}
```

### 3. Strategy Pattern with Trait Objects

**Use Case**: Runtime algorithm selection.

```rust
pub trait CompressionStrategy {
    fn compress(&self, data: &[u8]) -> Vec<u8>;
    fn decompress(&self, data: &[u8]) -> Vec<u8>;
}

pub struct GzipCompression;
pub struct ZstdCompression;

impl CompressionStrategy for GzipCompression {
    fn compress(&self, data: &[u8]) -> Vec<u8> {
        // Gzip compression implementation
        data.to_vec() // Placeholder
    }
    
    fn decompress(&self, data: &[u8]) -> Vec<u8> {
        // Gzip decompression implementation
        data.to_vec() // Placeholder
    }
}

impl CompressionStrategy for ZstdCompression {
    fn compress(&self, data: &[u8]) -> Vec<u8> {
        // Zstd compression implementation
        data.to_vec() // Placeholder
    }
    
    fn decompress(&self, data: &[u8]) -> Vec<u8> {
        // Zstd decompression implementation
        data.to_vec() // Placeholder
    }
}

pub struct FileProcessor {
    strategy: Box<dyn CompressionStrategy>,
}

impl FileProcessor {
    pub fn new(strategy: Box<dyn CompressionStrategy>) -> Self {
        Self { strategy }
    }
    
    pub fn process_file(&self, data: &[u8]) -> Vec<u8> {
        self.strategy.compress(data)
    }
}

// Usage
let processor = FileProcessor::new(Box::new(GzipCompression));
```

---

## Anti-Patterns

### 1. Clone to Satisfy Borrow Checker

**Problem**: Cloning everything to avoid borrow checker issues.

```rust
// ❌ Bad - Unnecessary clones
fn process_data(data: &Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for item in data {
        let cloned_item = item.clone(); // Unnecessary clone
        result.push(cloned_item.to_uppercase());
    }
    result
}

// ✅ Good - Work with references
fn process_data_good(data: &[String]) -> Vec<String> {
    data.iter()
        .map(|item| item.to_uppercase())
        .collect()
}
```

### 2. Deref Polymorphism

**Problem**: Overusing `Deref` trait for inheritance-like behavior.

```rust
use std::ops::Deref;

// ❌ Bad - Using Deref for inheritance
struct Manager {
    employee: Employee,
    team_size: usize,
}

impl Deref for Manager {
    type Target = Employee;
    
    fn deref(&self) -> &Self::Target {
        &self.employee
    }
}

// ✅ Good - Use composition and explicit delegation
impl Manager {
    pub fn employee(&self) -> &Employee {
        &self.employee
    }
    
    pub fn name(&self) -> &str {
        self.employee.name()
    }
    
    pub fn team_size(&self) -> usize {
        self.team_size
    }
}
```

### 3. Overusing `unwrap()`

**Problem**: Using `unwrap()` everywhere instead of proper error handling.

```rust
// ❌ Bad - Panic-prone code
fn process_config(path: &str) -> Config {
    let content = std::fs::read_to_string(path).unwrap();
    let config: Config = serde_json::from_str(&content).unwrap();
    config
}

// ✅ Good - Proper error handling
fn process_config_safe(path: &str) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)
        .map_err(ConfigError::IoError)?;
    let config: Config = serde_json::from_str(&content)
        .map_err(ConfigError::ParseError)?;
    Ok(config)
}

#[derive(Debug)]
enum ConfigError {
    IoError(std::io::Error),
    ParseError(serde_json::Error),
}
```

---

## Error Handling

### 1. Custom Error Types with `thiserror`

**Pattern**: Use `thiserror` for clean error definitions.

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Network request failed: {0}")]
    NetworkError(#[from] reqwest::Error),
    
    #[error("Authentication failed")]
    AuthenticationError,
    
    #[error("Rate limit exceeded. Try again in {retry_after} seconds")]
    RateLimitError { retry_after: u64 },
    
    #[error("Invalid input: {message}")]
    ValidationError { message: String },
}

// Usage
fn make_api_call() -> Result<ApiResponse, ApiError> {
    if !is_authenticated() {
        return Err(ApiError::AuthenticationError);
    }
    
    let response = reqwest::get("https://api.example.com/data")?;
    // ... process response
    Ok(response)
}
```

### 2. Error Conversion and Context

**Pattern**: Use `anyhow` for application errors with context.

```rust
use anyhow::{Context, Result};

fn process_user_file(user_id: u64) -> Result<UserData> {
    let path = format!("users/{}.json", user_id);
    
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read user file: {}", path))?;
    
    let user_data: UserData = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse user data for user {}", user_id))?;
    
    Ok(user_data)
}
```

### 3. Result Extensions

**Pattern**: Extend `Result` with custom methods for better ergonomics.

```rust
pub trait ResultExt<T> {
    fn log_error(self) -> Self;
    fn or_default_with<F>(self, f: F) -> T 
    where 
        F: FnOnce() -> T;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: std::fmt::Display,
{
    fn log_error(self) -> Self {
        if let Err(ref e) = self {
            eprintln!("Error: {}", e);
        }
        self
    }
    
    fn or_default_with<F>(self, f: F) -> T 
    where 
        F: FnOnce() -> T,
    {
        self.unwrap_or_else(|_| f())
    }
}

// Usage
let config = load_config()
    .log_error()
    .or_default_with(|| Config::default());
```

---

## API Design

### 1. Accept Borrowed Types in Function Parameters

**Pattern**: Use `&str` instead of `&String`, `&[T]` instead of `&Vec<T>`.

```rust
// ✅ Good - Flexible parameter types
fn process_text(text: &str) -> String {
    text.to_uppercase()
}

fn process_items<T>(items: &[T]) -> usize 
where
    T: std::fmt::Debug,
{
    for item in items {
        println!("{:?}", item);
    }
    items.len()
}

// ❌ Bad - Restrictive parameter types  
fn process_text_bad(text: &String) -> String {
    text.to_uppercase()
}

fn process_items_bad<T>(items: &Vec<T>) -> usize 
where
    T: std::fmt::Debug,
{
    items.len()
}
```

### 2. Use Trait Bounds for Generic Parameters

**Pattern**: Make generic functions more flexible with trait bounds.

```rust
// ✅ Good - Flexible with trait bounds
fn save_to_file<P, C>(path: P, content: C) -> std::io::Result<()>
where
    P: AsRef<Path>,
    C: AsRef<str>,
{
    std::fs::write(path, content.as_ref())
}

// Can be called with various types:
save_to_file("config.json", config_string)?;
save_to_file(&path_buf, &content)?;
save_to_file(Path::new("data.txt"), format!("Data: {}", value))?;
```

### 3. Provide Multiple Constructor Patterns

**Pattern**: Offer various ways to create instances.

```rust
pub struct DatabaseConnection {
    url: String,
    pool_size: usize,
    timeout: Duration,
}

impl DatabaseConnection {
    // Basic constructor
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            pool_size: 10,
            timeout: Duration::from_secs(30),
        }
    }
    
    // From configuration
    pub fn from_config(config: &DatabaseConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            url: config.url.clone(),
            pool_size: config.pool_size.unwrap_or(10),
            timeout: Duration::from_secs(config.timeout_seconds.unwrap_or(30)),
        })
    }
    
    // From environment
    pub fn from_env() -> Result<Self, std::env::VarError> {
        Ok(Self {
            url: std::env::var("DATABASE_URL")?,
            pool_size: std::env::var("DB_POOL_SIZE")
                .map(|s| s.parse().unwrap_or(10))
                .unwrap_or(10),
            timeout: Duration::from_secs(30),
        })
    }
}
```

---

## Concurrency

### 1. Async Error Handling

**Pattern**: Proper error handling in async contexts.

```rust
use tokio::time::{timeout, Duration};
use anyhow::{Context, Result};

pub struct AsyncHttpClient {
    client: reqwest::Client,
    base_url: String,
}

impl AsyncHttpClient {
    pub async fn get<T>(&self, endpoint: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = format!("{}/{}", self.base_url, endpoint);
        
        let response = timeout(Duration::from_secs(10), async {
            self.client
                .get(&url)
                .send()
                .await
                .context("Failed to send request")?
                .error_for_status()
                .context("Request returned error status")
        })
        .await
        .context("Request timed out")??;
        
        let data = response
            .json::<T>()
            .await
            .context("Failed to parse response as JSON")?;
        
        Ok(data)
    }
}
```

### 2. Shared State with Arc and RwLock

**Pattern**: Safe concurrent access to shared data.

```rust
use std::sync::{Arc, RwLock};
use std::collections::HashMap;

#[derive(Clone)]
pub struct SharedCache<K, V>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
{
    data: Arc<RwLock<HashMap<K, V>>>,
}

impl<K, V> SharedCache<K, V>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
{
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub fn get(&self, key: &K) -> Option<V> {
        self.data.read().ok()?.get(key).cloned()
    }
    
    pub fn insert(&self, key: K, value: V) -> Result<(), std::sync::PoisonError<()>> {
        self.data.write()
            .map_err(|_| std::sync::PoisonError::new(()))?
            .insert(key, value);
        Ok(())
    }
    
    pub fn len(&self) -> usize {
        self.data.read().map(|data| data.len()).unwrap_or(0)
    }
}

// Usage in async context
#[tokio::main]
async fn main() {
    let cache: SharedCache<String, i32> = SharedCache::new();
    let cache_clone = cache.clone();
    
    // Spawn tasks that share the cache
    let handles: Vec<_> = (0..10).map(|i| {
        let cache = cache_clone.clone();
        tokio::spawn(async move {
            cache.insert(format!("key_{}", i), i).unwrap();
        })
    }).collect();
    
    for handle in handles {
        handle.await.unwrap();
    }
    
    println!("Cache size: {}", cache.len());
}
```

### 3. Channel-Based Communication

**Pattern**: Use channels for async task communication.

```rust
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum WorkerMessage {
    ProcessData { data: String, response: oneshot::Sender<String> },
    GetValues { response: oneshot::Sender<WorkerValues> },
    Shutdown,
}

#[derive(Debug)]
pub struct WorkerValues {
    pub processed_items: usize,
    pub errors: usize,
}

pub struct Worker {
    processed_items: usize,
    errors: usize,
}

impl Worker {
    pub fn spawn() -> mpsc::Sender<WorkerMessage> {
        let (tx, mut rx) = mpsc::channel(100);
        
        tokio::spawn(async move {
            let mut worker = Worker {
                processed_items: 0,
                errors: 0,
            };
            
            while let Some(message) = rx.recv().await {
                match message {
                    WorkerMessage::ProcessData { data, response } => {
                        let result = worker.process_data(data).await;
                        worker.processed_items += 1;
                        let _ = response.send(result);
                    }
                    WorkerMessage::GetValues { response } => {
                        let values = WorkerValues {
                            processed_items: worker.processed_items,
                            errors: worker.errors,
                        };
                        let _ = response.send(values);
                    }
                    WorkerMessage::Shutdown => {
                        println!("Worker shutting down");
                        break;
                    }
                }
            }
        });
        
        tx
    }
    
    async fn process_data(&mut self, data: String) -> String {
        // Simulate processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        format!("Processed: {}", data)
    }
}

// Usage
#[tokio::main]
async fn main() {
    let worker = Worker::spawn();
    
    // Send work to the worker
    let (response_tx, response_rx) = oneshot::channel();
    worker.send(WorkerMessage::ProcessData {
        data: "test data".to_string(),
        response: response_tx,
    }).await.unwrap();
    
    let result = response_rx.await.unwrap();
    println!("Result: {}", result);
    
    // Get values
    let (values_tx, values_rx) = oneshot::channel();
    worker.send(WorkerMessage::GetValues {
        response: values_tx,
    }).await.unwrap();
    
    let values = values_rx.await.unwrap();
    println!("Values: {:?}", values);
    
    // Shutdown
    worker.send(WorkerMessage::Shutdown).await.unwrap();
}
```

---

## Memory Management

### 1. RAII Pattern

**Pattern**: Resource management through destructors.

```rust
use std::fs::File;
use std::io::Write;

pub struct TempFile {
    file: File,
    path: std::path::PathBuf,
}

impl TempFile {
    pub fn new(filename: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(filename);
        let file = File::create(&path)?;
        Ok(Self { file, path })
    }
    
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.file.write_all(data)
    }
    
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // Clean up the temporary file when the struct is dropped
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// Usage - file is automatically cleaned up when temp_file goes out of scope
fn process_temp_data() -> std::io::Result<()> {
    let mut temp_file = TempFile::new("process_data.tmp")?;
    temp_file.write(b"temporary data")?;
    
    // Process the file...
    
    // File is automatically deleted when temp_file is dropped
    Ok(())
}
```

### 2. Cow (Clone on Write) Pattern

**Pattern**: Optimize for the common case of not needing to modify data.

```rust
use std::borrow::Cow;

fn normalize_path(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        // Need to modify - clone and replace
        Cow::Owned(path.replace('\\', "/"))
    } else {
        // No modification needed - borrow
        Cow::Borrowed(path)
    }
}

// Usage
let unix_path = "/home/user/file.txt";
let windows_path = "C:\\Users\\user\\file.txt";

let normalized_unix = normalize_path(unix_path); // Borrowed
let normalized_windows = normalize_path(windows_path); // Owned

println!("Unix: {}", normalized_unix);
println!("Windows: {}", normalized_windows);
```

---

## Code Organization

### 1. Module Structure

**Pattern**: Organize code with clear module boundaries.

```
src/
├── main.rs
├── lib.rs
├── config/
│   ├── mod.rs
│   ├── database.rs
│   └── server.rs
├── handlers/
│   ├── mod.rs
│   ├── auth.rs
│   ├── users.rs
│   └── admin.rs
├── models/
│   ├── mod.rs
│   ├── user.rs
│   └── session.rs
├── services/
│   ├── mod.rs
│   ├── auth_service.rs
│   └── user_service.rs
└── utils/
    ├── mod.rs
    ├── validation.rs
    └── crypto.rs
```

```rust
// src/lib.rs
pub mod config;
pub mod handlers;
pub mod models;
pub mod services;
pub mod utils;

pub use config::AppConfig;
pub use models::{User, Session};

// Re-export commonly used items
pub mod prelude {
    pub use crate::config::AppConfig;
    pub use crate::models::{User, Session};
    pub use crate::services::{AuthService, UserService};
    pub use anyhow::{Context, Result};
}
```

### 2. Feature Flags

**Pattern**: Use Cargo features for optional functionality.

```toml
# Cargo.toml
[features]
default = ["json"]
json = ["serde_json"]
yaml = ["serde_yaml"]
database = ["sqlx"]
redis = ["redis-rs"]
```

```rust
// src/serialization.rs
#[cfg(feature = "json")]
pub fn to_json<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}

#[cfg(feature = "yaml")]
pub fn to_yaml<T: serde::Serialize>(value: &T) -> Result<String, serde_yaml::Error> {
    serde_yaml::to_string(value)
}

// Conditional compilation for different backends
#[cfg(feature = "database")]
pub mod database {
    // Database-specific code
}

#[cfg(feature = "redis")]
pub mod cache {
    // Redis-specific code
}
```

### 3. Configuration Management

**Pattern**: Centralized, type-safe configuration.

```rust
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: Option<u32>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub file: Option<String>,
}

impl AppConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&content)?;
        Ok(config)
    }
    
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(AppConfig {
            server: ServerConfig {
                host: std::env::var("SERVER_HOST")
                    .unwrap_or_else(|_| "localhost".to_string()),
                port: std::env::var("SERVER_PORT")
                    .unwrap_or_else(|_| "8080".to_string())
                    .parse()?,
                workers: std::env::var("SERVER_WORKERS")
                    .ok()
                    .and_then(|s| s.parse().ok()),
            },
            database: DatabaseConfig {
                url: std::env::var("DATABASE_URL")?,
                max_connections: std::env::var("DB_MAX_CONNECTIONS")
                    .ok()
                    .and_then(|s| s.parse().ok()),
                timeout_seconds: std::env::var("DB_TIMEOUT")
                    .ok()
                    .and_then(|s| s.parse().ok()),
            },
            logging: LoggingConfig {
                level: std::env::var("LOG_LEVEL")
                    .unwrap_or_else(|_| "info".to_string()),
                file: std::env::var("LOG_FILE").ok(),
            },
        })
    }
    
    pub fn merged_with_env(mut self) -> anyhow::Result<Self> {
        if let Ok(host) = std::env::var("SERVER_HOST") {
            self.server.host = host;
        }
        if let Ok(port) = std::env::var("SERVER_PORT") {
            self.server.port = port.parse()?;
        }
        // ... merge other environment variables
        Ok(self)
    }
}
```

## Summary

This guide covers essential Rust patterns that promote:

- **Safety**: Proper error handling, memory management
- **Performance**: Zero-cost abstractions, efficient data structures
- **Maintainability**: Clear APIs, good organization
- **Concurrency**: Safe parallel programming patterns

Remember that patterns are tools - choose the right pattern for your specific use case, and don't over-engineer simple solutions.

## References

- [Rust Unofficial Patterns](https://rust-unofficial.github.io/patterns/)
- [The Rust Programming Language Book](https://doc.rust-lang.org/book/)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Effective Rust](https://www.lurklurk.org/effective-rust/)

---

# Rust's Block Pattern

John Nunley · December 18, 2025

rust
Here’s a little idiom that I haven’t really seen discussed anywhere, that I think makes Rust code much cleaner and more robust.

I don’t know if there’s an actual name for this idiom; I’m calling it the “block pattern” for lack of a better word. I find myself reaching for it frequently in code, and I think other Rust code could become cleaner if it followed this pattern. If there’s an existing name for this, please let me know!

The pattern comes from blocks in Rust being valid expressions. For example, this code:

let foo = { 1 + 2 };
…is equal to this code:

let foo = 1 + 2;
…which is, in turn, equal to this code:

let foo = {
    let x = 1;
    let y = 2;
    x + y
};
So, why does this matter?

Let’s say you have a function that loads a configuration file, then sends a few HTTP requests based on that config file. In order to load that config file, first you need to load the raw bytes of that file from the disk. Then you need to parse whatever the format of the configuration file is. For the sake of having a complex enough program to demonstrate the value of this pattern, let’s say it’s JSON with comments. You would need to remove the comments first using the regex crate, then parse the resulting JSON with something like serde-json.

Such a function would look like this:

use regex::{Regex, RegexBuilder};
use std::{fs, sync::LazyLock};

/// Format of the configuration file.
#[derive(serde::Deserialize)]
struct Config { /* ... */ }

// Always make sure to cache your regexes!
static STRIP_COMMENTS: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"//.*").multi_line(true).build().expect("regex build failed")
});

/// Function to load the config and send some HTTP requests.
fn foo(cfg_file: &str) -> anyhow::Result<()> {
    // Load the raw bytes of the file.
    let config_data = fs::read(cfg_file)?;

    // Convert to a string to the regex can work on it.
    let config_string = String::from_utf8(&config_data)?;

    // Strip out all comments.
    let stripped_data = STRIP_COMMENTS.replace(&config_string, "");

    // Parse as JSON.
    let config = serde_json::from_str(&stripped_data)?;

    // Do some work based on this data.
    send_http_request(&config.url1)?;
    send_http_request(&config.url2)?;
    send_http_request(&config.url3)?;

    Ok(())
}
This is fairly simple, and just leverages a few Rust crates and language features to parse JSON and then do something with it.

However, there are a few weaknesses here. In the foo function, we declare four new variables (config_data, config_string, stripped_data, config) only for only one of those variables to be used after the configuration parsing (config). In addition, let’s say you didn’t know what this code was for going in, and you didn’t have these comments (or you had bad comments). One might ask why you’re declaring the regular expression STRIP_COMMENTS, or why you’re loading data from a file.

When I write code, I try to make it immediately obvious what the purpose of the code is, and why it’s written that way. This is why I generally avoid C’s “bottom-up” strategy for organizing code. It’s like being given a few screws and being expected to implicitly understand that it should be built into a chair. In Rust, I like that you are able to define your top-level functions first, and then go down and define all the bits and pieces after.

Although, we can do a little bit better. What if we organized the foo function like this:

/// Function to load the config and send some HTTP requests.
fn foo(cfg_file: &str) -> anyhow::Result<()> {
    // Load the configuration from the file.
    let config = {
        // Cached regular expression for stripping comments.
        static STRIP_COMMENTS: LazyLock<Regex> = LazyLock::new(|| {
            RegexBuilder::new(r"//.*").multi_line(true).build().expect("regex build failed")
        });

        // Load the raw bytes of the file.
        let raw_data = fs::read(cfg_file)?;

        // Convert to a string to the regex can work on it.
        let data_string = String::from_utf8(&raw_data)?;

        // Strip out all comments.
        let stripped_data = STRIP_COMMENTS.replace(&config_string, "");

        // Parse as JSON.
        serde_json::from_str(&stripped_data)?
    };

    // Do some work based on this data.
    send_http_request(&config.url1)?;
    send_http_request(&config.url2)?;
    send_http_request(&config.url3)?;

    Ok(())
}
In this function, we’ve moved all of the configuration-related code (parsing, loading, even the static regex) into the block. This works because Rust lets you have items, statements and expressions inside of a block, hence why we were able to move everything inside. This pattern has three immediate advantages:

The block starts with the intent of the code (let config = ...). We can see that we’re working to resolve some kind of configuration object right off the bat. Only then do we move into the implementation details of the code.
It reduces pollution of the namespace of both the foo function and the top-level module. Now in foo, the variable names config_data, config_string et al are no longer used. In addition to allowing these variable names to be re-used, it makes this code a lot more “idiot-proof”. If someone else were to edit the foo function, they would only be able to use config. They wouldn’t be able to use the raw_data or STRIP_COMMENTS items, which are only meant to be used by the config parser.
The variables raw_data and data_string go out of scope at the end of the block, which means they are dropped, freeing up resources.
As an aside, all three of these advantages also come if you were to refactor the block out into its own function. However, this pattern has two key advantages over that:

The code flow is still inline with the rest of the function. For shorter blocks, this improves reading comprehension, since it means you don’t have to go to a different part of the code to fully understand the function.
If there are a lot of variables that the block would use, it prevents needing to explicitly name those variables as parameters.
There is one more benefit that’s not exposed in the above example: erasure of mutability. Let’s say you construct some object for use in a later part of the function:

let mut data = vec![];
data.push(1);
data.extend_from_slice(&[4, 5, 6, 7]);

data.iter().for_each(|x| println!("{x}"));
return data[2];
The issue is that data is declared as mutable, which means the rest of the function can mutate it. Since a lot of bugs come from data being mutated when it isn’t supposed to be mutated, we’d like to restrict the mutability of the data to a certain area of the function. This is also possible with the block pattern:

let data = {
    let mut data = vec![];
    data.push(1);
    data.extend_from_slice(&[4, 5, 6, 7]);
    data
};

data.iter().for_each(|x| println!("{x}"));
return data[2];
This effectively “closes” the mutability to a certain section of the function.

Closing Thoughts

I don’t know if this pattern is already well known to the Rust community. Even if it isn’t, I figure it’s still a good idea to bring it to people who may be inexperienced in Rust.

Share: Twitter, Facebook

This website's source code is hosted via Codeberg

Any and all opinions expressed above are my own and not representative of any of my employers, past present and/or future.

---

## Field Examples — Patterns Rounded in Practice

*Added 2026-01-02*

These examples were extracted from a working Rust codebase (a game
engine) during phronesis's own development. They are illustrative of
the rules above and useful as before/after templates when authoring
similar rules of your own.

### Patterns Worth Borrowing

#### 1. Newtype Pattern (Opportunity)

The codebase currently uses `String` for state and slot IDs. Consider using newtypes for type safety:

```rust
// Current (in src/topology/graph.rs)
pub struct State {
    pub id: String,  // Easy to mix up with opponent_id or other strings
}

// Recommended improvement
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StateId(String);

impl StateId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

#### 2. Builder Pattern (Used in TensionConfig)

See `src/tension/pool.rs` for the TensionConfig builder:

```rust
// Example from the codebase
let config = TensionConfig::default()
    .with_auto_roll_threshold(6)
    .with_complication_trigger_value(6);
```

#### 3. Block Pattern (Opportunity)

Configuration loading in `src/core.rs` could benefit from the block pattern:

```rust
// Current pattern (spread across function)
let config_path = PathBuf::from(path);
let config_content = fs::read_to_string(&config_path)?;
let module_config: ModuleConfig = serde_yaml::from_str(&config_content)?;

// Improved with block pattern
let module_config = {
    let content = fs::read_to_string(path)?;
    serde_yaml::from_str::<ModuleConfig>(&content)?
};
// config_path and config_content no longer pollute namespace
```

#### 4. RAII Pattern (Used in SecretPassageTracker)

See `src/movement/secret.rs` for automatic discovery tracking:

```rust
// When SecretDiscovery is created, it records the discovery time
let discovery = SecretDiscovery {
    id: secret_id.to_string(),
    from_state_id: from.to_string(),
    to_state_id: to.to_string(),
    discovery_method: method,
    discovery_time: chrono::Utc::now(),
    discovery_details: details,
};
```

### Improvement Opportunities

1. **Unused Imports**: Run `cargo fix` to auto-remove unused imports (45 warnings)
2. **Snake Case**: Fix `cardId` → `card_id` in `src/example/play.rs`
3. **Block Pattern**: Apply to config loading sections in `src/core.rs`
4. **Newtype Pattern**: Create `StateId`, `SlotId`, `ItemId` for type safety
