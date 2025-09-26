use ahash::AHashMap;

use crate::StorePath;

#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub path: Option<StorePath>,
    pub hash: Option<String>,
    pub hash_algo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DerivationEnv {
    inner: AHashMap<String, String>,
}

impl DerivationEnv {
    fn new(v: AHashMap<String, String>) -> Self {
        Self { inner: v }
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.inner.get(k).map(|v| v.as_str())
    }

    pub fn get_required_system_features(&self) -> Vec<&str> {
        self.inner
            .get("requiredSystemFeatures")
            .map(|v| v.as_str())
            .unwrap_or_default()
            .split(' ')
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn get_output_hash(&self) -> Option<&str> {
        self.inner.get("outputHash").map(|v| v.as_str())
    }

    pub fn get_output_hash_mode(&self) -> Option<&str> {
        self.inner.get("outputHash").map(|v| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Derivation {
    pub env: DerivationEnv,
    pub input_drvs: Vec<String>,
    pub outputs: Vec<Output>,
    pub name: String,
    pub system: String,
}

impl Derivation {
    fn new(path: String, v: nix_diff::types::Derivation) -> Result<Self, std::str::Utf8Error> {
        Ok(Self {
            env: DerivationEnv::new(
                v.env
                    .into_iter()
                    .filter_map(|(k, v)| {
                        Some((String::from_utf8(k).ok()?, String::from_utf8(v).ok()?))
                    })
                    .collect(),
            ),
            input_drvs: v
                .input_derivations
                .into_keys()
                .filter_map(|v| String::from_utf8(v).ok())
                .collect(),
            outputs: v
                .outputs
                .into_iter()
                .filter_map(|(k, v)| {
                    Some(Output {
                        name: String::from_utf8(k).ok()?,
                        path: if v.path.is_empty() {
                            None
                        } else {
                            String::from_utf8(v.path).ok().map(|p| StorePath::new(&p))
                        },
                        hash: v.hash.map(String::from_utf8).transpose().ok()?,
                        hash_algo: v.hash_algorithm.map(String::from_utf8).transpose().ok()?,
                    })
                })
                .collect(),
            name: path,
            system: String::from_utf8(v.platform).unwrap_or_default(),
        })
    }
}

#[tracing::instrument(fields(%drv), err)]
pub async fn query_drv(drv: &StorePath) -> Result<Option<Derivation>, crate::Error> {
    if !drv.is_drv() {
        return Ok(None);
    }

    let full_path = drv.get_full_path();
    if !tokio::fs::try_exists(&full_path).await? {
        return Ok(None);
    }

    let input = tokio::fs::read_to_string(&full_path).await?;
    Ok(Some(Derivation::new(
        full_path,
        nix_diff::parser::parse_derivation_string(&input)?,
    )?))
}
