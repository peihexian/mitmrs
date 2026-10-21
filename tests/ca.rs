use mitmrs::ca::{CertAuthority, CA_VALIDITY_DAYS};

#[tokio::test]
async fn generated_root_ca_is_valid_for_ten_years() {
    let dir = tempfile::tempdir().unwrap();
    let ca = CertAuthority::load_or_create(dir.path().join("ca"))
        .await
        .unwrap();
    let days = (ca.not_after() - ca.not_before()).num_days();
    assert_eq!(days, CA_VALIDITY_DAYS);
    assert_eq!(CA_VALIDITY_DAYS, 3650);
    let pem = ca.pem();
    assert!(pem.contains("BEGIN CERTIFICATE"));
    assert!(ca.cert_path().exists());
}
