use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime};

/// Default root CA lifetime. The generated root is valid for exactly ten
/// years (3650 days), with `not_before` backdated by one day for clock skew.
pub const CA_VALIDITY_DAYS: i64 = 3650;

pub struct CertAuthority {
    issuer: Issuer<'static, KeyPair>,
    ca_pem: String,
    ca_der: Vec<u8>,
    fingerprint: String,
    dir: PathBuf,
    cache: Mutex<HashMap<String, Arc<ServerConfig>>>,
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
}

impl CertAuthority {
    pub async fn load_or_create(dir: PathBuf) -> Result<Self> {
        tokio::task::spawn_blocking(move || Self::load_or_create_blocking(dir)).await?
    }

    fn load_or_create_blocking(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let cert_path = dir.join("ca.crt");
        let der_path = dir.join("ca.der");
        let key_path = dir.join("ca.key");
        let meta_path = dir.join("ca.json");

        if cert_path.exists() && key_path.exists() && der_path.exists() {
            let ca_pem = std::fs::read_to_string(&cert_path)?;
            let key_pem = std::fs::read_to_string(&key_path)?;
            let key = KeyPair::from_pem(&key_pem).context("parse CA private key")?;
            let issuer = Issuer::from_ca_cert_pem(&ca_pem, key).context("parse CA certificate")?;
            let ca_der = std::fs::read(&der_path)?;
            let fingerprint = fingerprint(&ca_der);
            let meta: serde_json::Value = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let not_before = meta["not_before"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            let not_after = meta["not_after"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|| Utc::now() + chrono::Duration::days(CA_VALIDITY_DAYS));
            return Ok(Self {
                issuer,
                ca_pem,
                ca_der,
                fingerprint,
                dir,
                cache: Mutex::new(HashMap::new()),
                not_before,
                not_after,
            });
        }

        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "MitmRs");
        params
            .distinguished_name
            .push(DnType::CommonName, "MitmRs Local CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        // Exactly 10 years of certificate validity, including the one-day
        // backdate used for clock skew.
        params.not_after = params.not_before + Duration::days(CA_VALIDITY_DAYS);
        let key = KeyPair::generate().context("generate CA key")?;
        let cert = params.self_signed(&key).context("self-sign CA")?;
        let ca_pem = cert.pem();
        let ca_der = cert.der().to_vec();
        std::fs::write(&cert_path, &ca_pem)?;
        std::fs::write(&der_path, &ca_der)?;
        std::fs::write(&key_path, key.serialize_pem())?;
        let not_before = Utc::now() - chrono::Duration::days(1);
        let not_after = not_before + chrono::Duration::days(CA_VALIDITY_DAYS);
        let meta = serde_json::json!({
            "subject": "CN=MitmRs Local CA, O=MitmRs",
            "not_before": not_before.to_rfc3339(),
            "not_after": not_after.to_rfc3339(),
        });
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
        let fingerprint = fingerprint(&ca_der);
        let issuer = Issuer::new(params, key);
        tracing::info!("created local CA at {}", cert_path.display());
        Ok(Self {
            issuer,
            ca_pem,
            ca_der,
            fingerprint,
            dir,
            cache: Mutex::new(HashMap::new()),
            not_before,
            not_after,
        })
    }

    pub fn pem(&self) -> &str {
        &self.ca_pem
    }

    pub fn der(&self) -> &[u8] {
        &self.ca_der
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cert_path(&self) -> PathBuf {
        self.dir.join("ca.crt")
    }

    pub fn not_before(&self) -> DateTime<Utc> {
        self.not_before
    }

    pub fn not_after(&self) -> DateTime<Utc> {
        self.not_after
    }

    pub fn server_config_for(&self, host: &str) -> Result<Arc<ServerConfig>> {
        let key = normalize_host(host);
        if let Ok(cache) = self.cache.lock() {
            if let Some(config) = cache.get(&key) {
                return Ok(config.clone());
            }
        }
        let config = Arc::new(self.build_server_config(&key)?);
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() > 1024 {
                cache.clear();
            }
            cache.insert(key, config.clone());
        }
        Ok(config)
    }

    fn build_server_config(&self, host: &str) -> Result<ServerConfig> {
        let mut params = CertificateParams::new(vec![host.to_string()])
            .with_context(|| format!("invalid certificate host {host}"))?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, host);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let key = KeyPair::generate().context("generate leaf key")?;
        let cert = params
            .signed_by(&key, &self.issuer)
            .context("sign leaf certificate")?;
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .context("build TLS server config")?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(config)
    }
}

pub fn normalize_host(host: &str) -> String {
    let h = host.trim();
    let h = h.trim_start_matches('[').trim_end_matches(']');
    let h = h.split(':').next().unwrap_or(h);
    if h.is_empty() {
        "localhost".into()
    } else {
        h.to_ascii_lowercase()
    }
}

fn fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
