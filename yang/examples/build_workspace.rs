fn main() {
    let repo_root = yang::catalog::find_repo_root(std::env::current_dir().unwrap().as_path())
        .expect("could not find repo root (support/scripts/gen-cc-nodes.yaml not found in any ancestor)");
    println!("repo root: {}", repo_root.display());
    let files = yang::catalog::load_workspace(&repo_root).expect("load_workspace");
    println!("{} yang + {} sid files loaded", files.yang.len(), files.sid.len());
    let schema = yang::schema::build(&files.yang, &files.sid).expect("schema build");
    println!("OK: {} nodes, {} types, {} identities, {} SIDs indexed", schema.nodes.len(), schema.types.len(), schema.identities.len(), schema.sid_index.len());
}
