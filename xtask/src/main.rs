use std::process::Command;

fn main() {
    
    println!("=> cargo fmt --all --check");
    let status = Command::new("cargo")
        .args(["fmt", "--all", "--check"])
        .status()
        .expect("Failed to execute cargo fmt");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    
    println!("\n=> cargo clippy --features ssr --all-targets -- -D warnings");
    let status = Command::new("cargo")
        .args(["clippy", "--features", "ssr", "--all-targets", "--", "-D", "warnings"])
        .status()
        .expect("Failed to execute cargo clippy");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    
    println!("\n=> cargo test --features ssr");
    let status = Command::new("cargo")
        .args(["test", "--features", "ssr"])
        .status()
        .expect("Failed to execute cargo test");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    
    println!("\n=> ✅ CI checks passed!");
}
