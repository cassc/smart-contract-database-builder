use eyre::Result;
use foundry_compilers::solc::Solc;
use log::debug;
use regex::Regex;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

const VERSIONS_URL: &str = "https://binaries.soliditylang.org/linux-amd64/list.json";

/// Hashing the content after removing all the whitespaces
pub(crate) fn simple_hash(content: &str) -> String {
    let re = Regex::new(r"\s+").unwrap();
    let result = re.replace_all(content, "");
    let digest = md5::compute(result.as_bytes());
    format!("{:x}", digest)
}

#[derive(Deserialize)]
struct SolcVersion {
    version: String,
}

#[derive(Deserialize)]
struct SolcVersions {
    builds: Vec<SolcVersion>,
}

pub async fn download_all_solc_versions() -> Result<()> {
    // Create a HTTP client
    let client = Client::new();

    // Fetch the list of versions
    let response = client.get(VERSIONS_URL).send().await?.text().await?;
    let versions: SolcVersions = serde_json::from_str(&response)?;

    // Download each version
    for version in versions.builds {
        debug!("Downloading solc version {}", version.version);
        let version = version.version;
        let version = Version::parse(&version)?;
        let version = Version::new(version.major, version.minor, version.patch);
        Solc::find_or_install(&version)?;
    }

    debug!("All solc versions have been downloaded");
    Ok(())
}
