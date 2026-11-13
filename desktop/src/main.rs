#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kubeloupe::{get_namespaces, get_nodes, get_pods, NamespaceInfo, NodeInfo, PodInfo};

#[tauri::command]
async fn cmd_list_pods(namespace: Option<String>) -> Result<Vec<PodInfo>, String> {
    get_pods(namespace.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_namespaces() -> Result<Vec<NamespaceInfo>, String> {
    get_namespaces().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_nodes() -> Result<Vec<NodeInfo>, String> {
    get_nodes().await.map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            cmd_list_pods,
            cmd_list_namespaces,
            cmd_list_nodes
        ])
        .run(tauri::generate_context!())
        .expect("error while running kubeloupe");
}
