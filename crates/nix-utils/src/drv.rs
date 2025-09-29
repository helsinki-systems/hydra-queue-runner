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
pub struct CAOutput {
    pub name: String,
    pub path: StorePath,
    pub hash: String,
    pub hash_algo: String,
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
                        hash: v
                            .hash
                            .map(String::from_utf8)
                            .transpose()
                            .ok()?
                            .and_then(|v| if v.is_empty() { None } else { Some(v) }),
                        hash_algo: v
                            .hash_algorithm
                            .map(String::from_utf8)
                            .transpose()
                            .ok()?
                            .and_then(|v| if v.is_empty() { None } else { Some(v) }),
                    })
                })
                .collect(),
            name: path,
            system: String::from_utf8(v.platform).unwrap_or_default(),
        })
    }

    pub fn is_ca(&self) -> bool {
        self.outputs
            .iter()
            .any(|o| o.hash.is_some() && o.hash_algo.is_some())
    }

    pub fn get_ca_output(&self) -> Option<CAOutput> {
        self.outputs.iter().find_map(|o| {
            Some(CAOutput {
                path: o.path.clone()?,
                hash: o.hash.clone()?,
                hash_algo: o.hash_algo.clone()?,
                name: o.name.clone(),
            })
        })
    }
}

fn parse_drv(path: String, input: &str) -> Result<Derivation, crate::Error> {
    Ok(Derivation::new(
        path,
        nix_diff::parser::parse_derivation_string(input)?,
    )?)
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
    Ok(Some(parse_drv(full_path, &input)?))
}

#[cfg(test)]
mod tests {
    use crate::{StorePath, drv::parse_drv};

    #[test]
    fn test_ca_derivation() {
        let drv_str = r#"Derive([("out","/nix/store/6fr8dalasgpy0bpykhjq2b9q65lb4j8y-linux-6.16.tar.xz","sha256","1a4be2fe6b5246aa4ac8987a8a4af34c42a8dd7d08b46ab48516bcc1befbcd83")],[],[],"builtin","builtin:fetchurl",[],[("builder","builtin:fetchurl"),("executable",""),("impureEnvVars","http_proxy https_proxy ftp_proxy all_proxy no_proxy"),("name","linux-6.16.tar.xz"),("out","/nix/store/6fr8dalasgpy0bpykhjq2b9q65lb4j8y-linux-6.16.tar.xz"),("outputHash","sha256-Gkvi/mtSRqpKyJh6ikrzTEKo3X0ItGq0hRa8wb77zYM="),("outputHashAlgo",""),("outputHashMode","flat"),("preferLocalBuild","1"),("system","builtin"),("unpack",""),("url","https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.16.tar.xz"),("urls","https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.16.tar.xz")])"#;
        let drv = parse_drv(
            "/nix/store/awmdz2lkxkdqnhdhk09zy9w7kzpl8jhc-linux-6.16.tar.xz.drv".into(),
            drv_str,
        )
        .unwrap();
        assert!(drv.is_ca());
        let o = drv.get_ca_output().unwrap();
        assert_eq!(
            o.path,
            StorePath::new("/nix/store/6fr8dalasgpy0bpykhjq2b9q65lb4j8y-linux-6.16.tar.xz")
        );
        assert_eq!(o.name, String::from("out"));
        assert_eq!(
            o.hash,
            String::from("1a4be2fe6b5246aa4ac8987a8a4af34c42a8dd7d08b46ab48516bcc1befbcd83")
        );
        assert_eq!(o.hash_algo, String::from("sha256"));
    }

    #[test]
    fn test_no_ca_derivation() {
        let drv_str = r#"Derive([("info","/nix/store/50m785l8aaqhy28h0jwi0rd02wjlrzb4-gnused-4.9-info","",""),("out","/nix/store/pmhkmqy0vxk47r6ndh0azybhf6gs6k25-gnused-4.9","","")],[("/nix/store/05q48dcd4lgk4vh7wyk330gr2fr082i2-bootstrap-tools.drv",["out"]),("/nix/store/d7ir44pmpq2y8lyyrniiflkw9cw0a5k3-bootstrap-stage4-stdenv-linux.drv",["out"]),("/nix/store/hxn5cn3r48d4css91508lgkd5vwnnqcr-update-autotools-gnu-config-scripts-hook.drv",["out"]),("/nix/store/lmwgsavisgsqql05kkl91v82psa1q191-perl-5.40.0.drv",["out"]),("/nix/store/ql7jhf1wzhazxw4cw627zyls49xm5g3n-sed-4.9.tar.xz.drv",["out"])],["/nix/store/l622p70vy8k5sh7y5wizi5f2mic6ynpg-source-stdenv.sh","/nix/store/shkw4qm9qcw5sc5n1k5jznc83ny02r39-default-builder.sh"],"x86_64-linux","/nix/store/razasrvdg7ckplfmvdxv4ia3wbayr94s-bootstrap-tools/bin/bash",["-e","/nix/store/l622p70vy8k5sh7y5wizi5f2mic6ynpg-source-stdenv.sh","/nix/store/shkw4qm9qcw5sc5n1k5jznc83ny02r39-default-builder.sh"],[("NIX_MAIN_PROGRAM","sed"),("__structuredAttrs",""),("buildInputs",""),("builder","/nix/store/razasrvdg7ckplfmvdxv4ia3wbayr94s-bootstrap-tools/bin/bash"),("cmakeFlags",""),("configureFlags",""),("depsBuildBuild",""),("depsBuildBuildPropagated",""),("depsBuildTarget",""),("depsBuildTargetPropagated",""),("depsHostHost",""),("depsHostHostPropagated",""),("depsTargetTarget",""),("depsTargetTargetPropagated",""),("doCheck",""),("doInstallCheck",""),("info","/nix/store/50m785l8aaqhy28h0jwi0rd02wjlrzb4-gnused-4.9-info"),("mesonFlags",""),("name","gnused-4.9"),("nativeBuildInputs","/nix/store/jwjq0fjgn7d00kswhaw2m8hbgws5vbi4-update-autotools-gnu-config-scripts-hook /nix/store/jvq4g81anbq7d0pwrrmgpi6721akly3h-perl-5.40.0"),("out","/nix/store/pmhkmqy0vxk47r6ndh0azybhf6gs6k25-gnused-4.9"),("outputs","out info"),("patches",""),("pname","gnused"),("preConfigure","patchShebangs ./build-aux/help2man"),("propagatedBuildInputs",""),("propagatedNativeBuildInputs",""),("src","/nix/store/a2i83vprqrd9nkr116yy1ksrwz5y1vq9-sed-4.9.tar.xz"),("stdenv","/nix/store/dlbnrl548f1vq1d8x6124p21p8alirxl-bootstrap-stage4-stdenv-linux"),("strictDeps",""),("system","x86_64-linux"),("version","4.9")])"#;
        let drv = parse_drv(
            "/nix/store/whmvyjphw10d78zfrb04kqjlc0dy68z3-gnused-4.9.drv".into(),
            drv_str,
        )
        .unwrap();
        assert!(!drv.is_ca());
    }
}
