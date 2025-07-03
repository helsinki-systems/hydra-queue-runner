use tokio::io::{AsyncBufReadExt as _, BufReader};

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

pub async fn parse_nix_support_from_outputs(outputs: &[&str]) -> std::io::Result<NixSupport> {
    let mut metrics = Vec::new();
    let mut failed = false;
    let mut hydra_release_name = None;

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-metrics");

        if let Ok(file) = tokio::fs::File::open(&file_path).await {
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
    }

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/failed");
        if tokio::fs::try_exists(file_path).await.unwrap_or_default() {
            failed = true;
            break;
        }
    }

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-release-name");
        if let Ok(v) = tokio::fs::read_to_string(file_path).await {
            let v = v.trim();
            if !v.is_empty() {
                hydra_release_name = Some(v.to_owned());
                break;
            }
        }
    }

    // TODO: parse products
    Ok(NixSupport {
        metrics,
        failed,
        hydra_release_name,
        products: vec![],
    })
}
