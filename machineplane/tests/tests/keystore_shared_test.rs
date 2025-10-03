use std::sync::{Arc, Mutex};
use tokio;
use env_logger;

#[tokio::test]
async fn test_keystore_shared_across_threads() {
    let _ = env_logger::try_init();
    
    // Set the environment variables
    std::env::set_var("BEEMESH_KEYSTORE_EPHEMERAL", "1");
    std::env::set_var("BEEMESH_KEYSTORE_SHARED_NAME", "test_shared");

    let result = Arc::new(Mutex::new(Vec::new()));

    // Thread 1: Store a keyshare
    let result1 = result.clone();
    let handle1 = tokio::spawn(async move {
        let keystore = crypto::open_keystore_default().expect("keystore open 1");
        let cid = "test_cid_123";
        let blob = b"test_blob_data";
        keystore.put(cid, blob, Some("test")).expect("keystore put");
        
        let cids = keystore.list_cids().expect("list cids 1");
        result1.lock().unwrap().push(format!("Thread1 stored and found: {:?}", cids));
    });

    // Thread 2: Try to read the keyshare
    let result2 = result.clone();
    let handle2 = tokio::spawn(async move {
        // Small delay to let thread 1 finish
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        let keystore = crypto::open_keystore_default().expect("keystore open 2");
        let cids = keystore.list_cids().expect("list cids 2");
        result2.lock().unwrap().push(format!("Thread2 found: {:?}", cids));
        
        if cids.contains(&"test_cid_123".to_string()) {
            let blob = keystore.get("test_cid_123").expect("get").expect("exists");
            result2.lock().unwrap().push(format!("Thread2 retrieved blob: {:?}", blob));
        }
    });

    handle1.await.unwrap();
    handle2.await.unwrap();

    let results = result.lock().unwrap();
    println!("Test results:");
    for (i, r) in results.iter().enumerate() {
        println!("  {}: {}", i+1, r);
    }
    
    // Check if thread 2 could see the data stored by thread 1
    let found_by_thread2 = results.iter().any(|r| r.contains("Thread2 found:") && r.contains("test_cid_123"));
    assert!(found_by_thread2, "Thread 2 should be able to see data stored by Thread 1 in shared keystore");
}