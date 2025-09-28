// // Example usage of the DHT-enabled libp2p mesh for storing and retrieving deployed manifests

// use serde_json::json;
// use crate::libp2p_beemesh::{
//     setup_libp2p_node, start_libp2p_node,
//     control::Libp2pControl,
//     dht_helpers::{store_manifest_in_dht, get_manifest_from_dht, bootstrap_dht}
// };

// /// Example of how to use the DHT-enabled mesh for storing applied manifests
// pub async fn example_dht_usage() -> Result<(), Box<dyn std::error::Error>> {
//     // 1. Setup the libp2p node with DHT support
//     let (swarm, topic, peer_rx, peer_tx) = setup_libp2p_node()?;
    
//     // 2. Create control channels
//     let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
    
//     // 3. Start the libp2p node in the background
//     let local_peer_id = *swarm.local_peer_id();
//     tokio::spawn(async move {
//         if let Err(e) = start_libp2p_node(swarm, topic, peer_tx, control_rx).await {
//             eprintln!("libp2p node error: {}", e);
//         }
//     });

//     // 4. Bootstrap the DHT
//     bootstrap_dht(&control_tx).await?;

//     // 5. Wait a bit for the network to stabilize
//     tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

//     // 6. Example: Deploy a manifest and store it in the DHT
//     let example_manifest = json!({
//         "apiVersion": "apps/v1",
//         "kind": "Deployment",
//         "metadata": {
//             "name": "nginx-deployment",
//             "namespace": "default"
//         },
//         "spec": {
//             "replicas": 3,
//             "selector": {
//                 "matchLabels": {
//                     "app": "nginx"
//                 }
//             },
//             "template": {
//                 "metadata": {
//                     "labels": {
//                         "app": "nginx"
//                     }
//                 },
//                 "spec": {
//                     "containers": [{
//                         "name": "nginx",
//                         "image": "nginx:1.21",
//                         "ports": [{
//                             "containerPort": 80
//                         }]
//                     }]
//                 }
//             }
//         }
//     });

//     // Store the manifest after "deployment"
//     let manifest_id = store_manifest_in_dht(
//         &control_tx,
//         "default".to_string(),           // tenant
//         "op-12345".to_string(),          // operation_id
//         local_peer_id,                   // local peer
//         example_manifest,                // the manifest
//         "Deployment".to_string(),        // manifest kind
//     ).await?;

//     println!("Stored manifest with ID: {}", manifest_id);

//     // 7. Example: Retrieve the manifest from the DHT
//     match get_manifest_from_dht(&control_tx, manifest_id.clone()).await? {
//         Some(manifest_bytes) => {
//             // Parse the retrieved manifest
//             match protocol::machine::root_as_applied_manifest(&manifest_bytes) {
//                 Ok(applied_manifest) => {
//                     println!("Retrieved manifest:");
//                     println!("  ID: {:?}", applied_manifest.id());
//                     println!("  Tenant: {:?}", applied_manifest.tenant());
//                     println!("  Kind: {:?}", applied_manifest.manifest_kind());
//                     println!("  Origin Peer: {:?}", applied_manifest.origin_peer());
//                     println!("  Timestamp: {}", applied_manifest.timestamp());
//                 }
//                 Err(e) => {
//                     println!("Failed to parse retrieved manifest: {:?}", e);
//                 }
//             }
//         }
//         None => {
//             println!("Manifest {} not found in DHT", manifest_id);
//         }
//     }

//     Ok(())
// }

// /// Example of how manifests are automatically stored when received via apply requests
// pub async fn example_automatic_dht_storage() {
//     println!("=== Automatic DHT Storage Example ===");
//     println!("When a peer receives an ApplyRequest:");
//     println!("1. The manifest is validated and deployed locally");
//     println!("2. If deployment succeeds, the applied manifest is automatically stored in DHT");
//     println!("3. Other peers can query the DHT to see what workloads are deployed");
//     println!("4. The DHT acts as a decentralized registry of all applied manifests");
    
//     println!("\n=== DHT Key Structure ===");
//     println!("Manifest records: manifest:<sha256-id>");
//     println!("Tenant indexes: tenant-index:<tenant-name>");
//     println!("Peer indexes: peer-index:<peer-id>");
    
//     println!("\n=== Benefits ===");
//     println!("- Decentralized: No single point of failure");
//     println!("- Discoverable: Any peer can query for deployed manifests");
//     println!("- Scalable: Distributed across all mesh participants");
//     println!("- Resilient: Multiple replicas across the network");
//     println!("- Content-addressable: Manifests can be found by content hash");
// }