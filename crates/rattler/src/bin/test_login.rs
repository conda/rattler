use std::process::Command;

fn call_login_via_binary(host: &str, token: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("../../target/release/rattler")
        .args(&["auth", "login", host, "--token", token])
        .output()?;

    if output.status.success() {
        println!(
            "Login successful: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    } else {
        eprintln!("Login failed: {}", String::from_utf8_lossy(&output.stderr));
        Err("Login command failed".into())
    }
}

fn main() {
    println!("=== Testing with valid token ===");
    match call_login_via_binary(
        "beta.prefix.dev",
        "pfx-6drYw0JqwdTUlyQP0jmr4sZtfXvPmxGXwMlf",
    ) {
        Ok(()) => println!("✅ Valid token test: Login succeeded!"),
        Err(e) => eprintln!("❌ Valid token test: Login failed: {}", e),
    }

    println!("\n=== Testing with invalid token ===");
    match call_login_via_binary("beta.prefix.dev", "invalid-token-12345") {
        Ok(()) => println!("✅ Invalid token test: Login succeeded! (This shouldn't happen)"),
        Err(e) => eprintln!("❌ Invalid token test: Login failed as expected: {}", e),
    }
}
