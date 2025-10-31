//! Verify Sigstore attestations for conda packages

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use sha2::{Digest, Sha256};
use sigstore::bundle::verify::Verifier as SigstoreVerifier;
use sigstore::bundle::Bundle;

/// Verify Sigstore attestations for a conda package
#[derive(Debug, Parser)]
pub struct Opt {
    /// URL to the conda package (e.g., https://prefix.dev/channel/linux-64/package.conda)
    /// The .sig file will be fetched automatically by appending .sig to the URL
    #[arg(required = true)]
    pub package_url: String,

    /// Show detailed certificate information
    #[arg(long)]
    pub verbose: bool,
}

/// Verify a package's Sigstore attestations
pub async fn verify(opts: Opt) -> miette::Result<()> {
    // Parse and handle .sig URL
    let mut package_url = url::Url::parse(&opts.package_url)
        .into_diagnostic()
        .context("Invalid package URL")?;

    // Check if URL already ends with .sig
    let sig_url = if package_url.path().ends_with(".sigs") {
        // Remove .sigs to get the package URL
        let path = package_url
            .path()
            .strip_suffix(".sigs")
            .unwrap()
            .to_string();
        package_url.set_path(&path);
        opts.package_url.clone()
    } else {
        format!("{}.sigs", package_url)
    };

    println!("Package URL: {}", package_url);
    println!("Fetching attestations from: {}", sig_url);
    println!();

    // Fetch the .sig file
    let client = reqwest::Client::new();
    let response = client
        .get(&sig_url)
        .send()
        .await
        .into_diagnostic()
        .context("Failed to fetch .sig file")?;

    if !response.status().is_success() {
        return Err(miette::miette!(
            "Failed to fetch .sig file: HTTP {}",
            response.status()
        ));
    }

    let sig_content = response
        .text()
        .await
        .into_diagnostic()
        .context("Failed to read .sig file content")?;

    // Parse the bundles (it's an array of Sigstore bundles)
    let bundles: Vec<Bundle> = serde_json::from_str(&sig_content)
        .into_diagnostic()
        .context("Failed to parse .sig file as JSON array of Sigstore bundles")?;

    if bundles.is_empty() {
        println!("⚠️  No attestations found in .sig file");
        return Ok(());
    }

    println!("Found {} attestation(s)", bundles.len());
    println!();

    // Fetch the actual package to verify against
    println!("Fetching package for verification...");
    let package_response = client
        .get(package_url.as_str())
        .send()
        .await
        .into_diagnostic()
        .context("Failed to fetch package")?;

    if !package_response.status().is_success() {
        return Err(miette::miette!(
            "Failed to fetch package: HTTP {}",
            package_response.status()
        ));
    }

    let package_bytes = package_response
        .bytes()
        .await
        .into_diagnostic()
        .context("Failed to read package content")?;

    // Compute package SHA256
    let mut hasher = Sha256::new();
    hasher.update(&package_bytes);
    let package_digest = hasher.finalize();

    println!("Package SHA256: {}", hex::encode(package_digest));
    println!();

    // Create Sigstore verifier with production trust root
    println!("Initializing Sigstore verifier...");
    let verifier = SigstoreVerifier::production()
        .await
        .into_diagnostic()
        .context("Failed to create Sigstore verifier")?;

    // Verify each bundle
    for (idx, bundle) in bundles.iter().enumerate() {
        println!("═══════════════════════════════════════════════════════════");
        println!("Attestation #{}", idx + 1);
        println!("═══════════════════════════════════════════════════════════");

        // Extract timestamp information from transparency log
        if let Some(tlog_entries) = bundle
            .verification_material
            .as_ref()
            .map(|vm| &vm.tlog_entries)
        {
            if !tlog_entries.is_empty() {
                println!();
                println!("Transparency Log:");
                for entry in tlog_entries {
                    if entry.integrated_time != 0 {
                        let dt = chrono::DateTime::from_timestamp(entry.integrated_time, 0)
                            .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);
                        println!("  Logged at: {}", dt.format("%Y-%m-%d %H:%M:%S UTC"));
                    }
                    if entry.log_index != 0 {
                        println!("  Log Index: {}", entry.log_index);
                    }
                }
            }
        }

        // Extract DSSE payload (the in-toto statement)
        if let Some(content) = &bundle.content {
            use sigstore_protobuf_specs::dev::sigstore::bundle::v1::bundle::Content;
            if let Content::DsseEnvelope(envelope) = content {
                let payload = &envelope.payload;
                if !payload.is_empty() {
                    // Try to parse directly as JSON first (in case it's already decoded)
                    let statement_result = serde_json::from_slice::<serde_json::Value>(payload)
                        .or_else(|first_err| {
                            // If that fails, try base64 decoding first
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD
                                .decode(payload)
                                .ok()
                                .and_then(|decoded| {
                                    serde_json::from_slice::<serde_json::Value>(&decoded).ok()
                                })
                                .ok_or(first_err)
                        });

                    match statement_result {
                        Ok(statement) => {
                            println!();
                            println!("In-toto Statement:");
                            if let Some(predicate_type) = statement.get("predicateType") {
                                println!("  Predicate Type: {}", predicate_type);
                            }
                            if let Some(subjects) =
                                statement.get("subject").and_then(|s| s.as_array())
                            {
                                println!("  Subjects:");
                                for subject in subjects {
                                    if let Some(name) = subject.get("name") {
                                        println!("    - {}", name);
                                    }
                                    if let Some(digest) = subject.get("digest") {
                                        if let Some(sha256) = digest.get("sha256") {
                                            println!("      SHA256: {}", sha256);
                                        }
                                    }
                                }
                            }
                            if let Some(predicate) = statement.get("predicate") {
                                println!(
                                    "  Predicate: {}",
                                    serde_json::to_string_pretty(predicate)
                                        .unwrap_or_else(|_| "{}".to_string())
                                );
                            }
                        }
                        Err(e) => {
                            println!("  ⚠️  Could not parse DSSE payload: {}", e);
                        }
                    }
                }
            }
        }

        // Perform cryptographic verification
        println!();
        println!("Cryptographic Verification:");

        // Create a policy that accepts any valid certificate from the trust root
        // We create a custom policy that always passes (accepts any certificate)
        struct UncheckedPolicy;
        impl sigstore::bundle::verify::VerificationPolicy for UncheckedPolicy {
            fn verify(
                &self,
                _cert: &x509_cert::Certificate,
            ) -> Result<(), sigstore::bundle::verify::policy::PolicyError> {
                Ok(())
            }
        }
        let policy = UncheckedPolicy;

        // Verify the bundle against the package digest
        match verifier
            .verify_digest_bytes(&package_digest.into(), bundle.clone(), &policy, false)
            .await
        {
            Ok(checked_bundle) => {
                println!("  ✓ Signature verification: PASSED");
                println!("  ✓ Certificate chain: VALID");
                println!("  ✓ Transparency log: VERIFIED");
                println!();

                // Extract and display certificate information
                extract_and_display_cert_info(checked_bundle.certificate(), opts.verbose)?;

                println!();
                println!("✅ VERIFICATION SUCCESSFUL");
            }
            Err(e) => {
                println!("  ❌ VERIFICATION FAILED");
                println!("  Error: {}", e);
                println!();
                println!("⚠️  This attestation could not be cryptographically verified");
            }
        }

        println!();
    }

    println!("═══════════════════════════════════════════════════════════");

    Ok(())
}

fn extract_and_display_cert_info(
    cert: &x509_cert::Certificate,
    verbose: bool,
) -> miette::Result<()> {
    use const_oid::ObjectIdentifier;
    use x509_cert::ext::pkix::{name::GeneralName, SubjectAltName};

    println!("Certificate Information:");

    // Extract Subject Alternative Name (identity)
    let tbs = &cert.tbs_certificate;
    if let Ok(Some((_, san))) = tbs.get::<SubjectAltName>() {
        println!("  Identity:");
        for name in &san.0 {
            match name {
                GeneralName::Rfc822Name(email) => {
                    println!("    Email: {}", email.as_str());
                }
                GeneralName::UniformResourceIdentifier(uri) => {
                    println!("    URI: {}", uri.as_str());
                }
                GeneralName::OtherName(other) => {
                    if let Ok(value) = std::str::from_utf8(other.value.value()) {
                        println!("    Other: {}", value);
                    }
                }
                _ => {}
            }
        }
    }

    // Extract OIDC extensions (GitHub Actions metadata)
    let extensions = tbs.extensions.as_deref().unwrap_or(&[]);

    // OIDC Issuer (1.3.6.1.4.1.57264.1.1)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.1"))
    {
        if let Ok(issuer) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  OIDC Issuer: {}", issuer);
        }
    }

    // GitHub Workflow Trigger (1.3.6.1.4.1.57264.1.2)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.2"))
    {
        if let Ok(trigger) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  GitHub Workflow Trigger: {}", trigger);
        }
    }

    // GitHub Workflow SHA (1.3.6.1.4.1.57264.1.3)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.3"))
    {
        if let Ok(sha) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  GitHub Commit SHA: {}", sha);
        }
    }

    // GitHub Workflow Name (1.3.6.1.4.1.57264.1.4)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.4"))
    {
        if let Ok(workflow) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  GitHub Workflow: {}", workflow);
        }
    }

    // GitHub Workflow Repository (1.3.6.1.4.1.57264.1.5)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.5"))
    {
        if let Ok(repo) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  GitHub Repository: {}", repo);
        }
    }

    // GitHub Workflow Ref (1.3.6.1.4.1.57264.1.6)
    if let Some(ext) = extensions
        .iter()
        .find(|e| e.extn_id == ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.6"))
    {
        if let Ok(git_ref) = std::str::from_utf8(ext.extn_value.as_bytes()) {
            println!("  GitHub Ref: {}", git_ref);
        }
    }

    if verbose {
        // Show certificate validity period
        println!("  Valid From: {}", tbs.validity.not_before);
        println!("  Valid To: {}", tbs.validity.not_after);
    }

    Ok(())
}
