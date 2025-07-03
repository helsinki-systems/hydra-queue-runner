use std::os::unix::fs::MetadataExt as _;

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

pub struct BuildProduct {
    pub path: String,
    pub default_path: String,

    pub r#type: String,
    pub subtype: String,
    pub name: String,

    pub is_regular: bool,

    pub sha256hash: Option<String>,
    pub file_size: Option<u64>,
}

pub struct BuildMetric {
    pub path: String,
    pub name: String,
    pub unit: Option<String>,
    pub value: f64,
}

pub struct NixSupport {
    pub failed: bool,
    pub hydra_release_name: Option<String>,
    pub metrics: Vec<BuildMetric>,
    pub products: Vec<BuildProduct>,
}

fn get_name_from_path(v: &str) -> String {
    let w = v.strip_prefix("/nix/store/").unwrap_or(v);
    if w.len() > 32 + 1 {
        w[32 + 1..].into()
    } else {
        String::new()
    }
}

pub async fn parse_nix_support_from_outputs(
    derivation_outputs: &[crate::DerivationOutput],
) -> std::io::Result<NixSupport> {
    let mut metrics = Vec::new();
    let mut failed = false;
    let mut hydra_release_name = None;

    let outputs = derivation_outputs
        .iter()
        .filter_map(|o| o.path.as_deref())
        .collect::<Vec<_>>();
    for output in &outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-metrics");
        let Ok(file) = tokio::fs::File::open(&file_path).await else {
            continue;
        };

        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            let fields: Vec<String> = line.split_whitespace().map(ToOwned::to_owned).collect();
            if fields.len() < 2 {
                continue;
            }

            metrics.push(BuildMetric {
                path: (*output).to_owned(),
                name: fields[0].clone(),
                value: fields[1].parse::<f64>().unwrap_or(0.0),
                unit: if fields.len() >= 3 {
                    Some(fields[2].clone())
                } else {
                    None
                },
            });
        }
    }

    for output in &outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/failed");
        if tokio::fs::try_exists(file_path).await.unwrap_or_default() {
            failed = true;
            break;
        }
    }

    for output in &outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-release-name");
        if let Ok(v) = tokio::fs::read_to_string(file_path).await {
            let v = v.trim();
            if !v.is_empty() {
                hydra_release_name = Some(v.to_owned());
                break;
            }
        }
    }

    let regex = regex::Regex::new(r"").unwrap();
    let mut explicit_products = false;
    let mut products = Vec::new();
    for output in &outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-build-products");
        let Ok(file) = tokio::fs::File::open(&file_path).await else {
            continue;
        };

        explicit_products = true;

        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            let Some(captures) = regex.captures(&line) else {
                continue;
            };

            let s = captures[3].to_string();
            let path = if s.starts_with('"') {
                s[1..s.len() - 1].to_string()
            } else {
                s
            };

            if path.is_empty() || !path.starts_with('/') {
                continue;
            }
            if !crate::check_if_storepath_exists(&path) {
                continue;
            }
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };
            let is_regular = metadata.is_file();

            let name = if &path == output {
                String::new()
            } else {
                std::path::Path::new(&path)
                    .file_name()
                    .and_then(|v| v.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_default()
            };

            let sha256hash = if is_regular {
                let mut file = tokio::fs::File::open(&path).await?;
                let mut sha256 = Sha256::new();

                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer).await?;
                sha256.update(&buffer);

                Some(format!("{:x}", sha256.finalize()))
            } else {
                None
            };

            products.push(BuildProduct {
                r#type: captures[1].to_string(),
                subtype: captures[2].to_string(),
                path,
                default_path: captures
                    .get(5)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                name,
                is_regular,
                file_size: if is_regular {
                    Some(metadata.size())
                } else {
                    None
                },
                sha256hash,
            });
        }
    }

    if !explicit_products {
        for o in derivation_outputs {
            let Some(path) = &o.path else {
                continue;
            };
            let Ok(metadata) = tokio::fs::metadata(path).await else {
                continue;
            };
            if metadata.is_dir() {
                products.push(BuildProduct {
                    r#type: "nix-build".to_string(),
                    subtype: if o.name == "out" {
                        String::new()
                    } else {
                        o.name.clone()
                    },
                    path: path.clone(),
                    name: get_name_from_path(path),
                    default_path: String::new(),
                    is_regular: false,
                    file_size: None,
                    sha256hash: None,
                });
            }
        }
    }

    Ok(NixSupport {
        metrics,
        failed,
        hydra_release_name,
        products,
    })
}
