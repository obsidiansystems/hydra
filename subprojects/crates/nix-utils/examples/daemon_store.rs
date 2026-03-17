#[tokio::main]
async fn main() {
    let store = nix_utils::daemon_store::DaemonLocalStore::new();
    println!("Store dir: {}", store.store_dir());

    let arg = std::env::args().nth(1).expect("usage: daemon_store <store-path>");
    let path = nix_utils::parse_store_path(&arg);
    println!("Checking: {}", store.store_dir().display(&path));

    let valid = store.is_valid_path(&path).await;
    println!("Valid: {valid}");

    if valid {
        if let Some(info) = store.query_path_info(&path).await {
            println!("NAR hash: {}", info.nar_hash);
            println!("NAR size: {}", info.nar_size);
            println!("Refs: {}", info.refs.len());
            for r in &info.refs {
                println!("  {r}");
            }
            if let Some(d) = &info.deriver {
                println!("Deriver: {d}");
            }
            if let Some(ca) = &info.ca {
                println!("CA: {ca}");
            }
            println!("Sigs: {}", info.sigs.len());
        }
    }

    // Test batch query
    let paths = vec![&path];
    let infos = store.query_path_infos(&paths).await;
    println!("\nBatch query returned {} results", infos.len());

    // Test ensure_path
    match store.ensure_path(&path).await {
        Ok(()) => println!("ensure_path: OK"),
        Err(e) => println!("ensure_path: {e}"),
    }
}
