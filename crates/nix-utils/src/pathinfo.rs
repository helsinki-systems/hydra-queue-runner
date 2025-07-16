use ahash::AHashMap;
use cached::proc_macro::cached;
use cached::{Cached, UnboundCache};

use crate::StorePath;

#[derive(Debug, serde::Deserialize)]
struct InnerPathInfo {
    ca: Option<String>,
    deriver: Option<String>,
    #[serde(rename = "narHash")]
    nar_hash: String,
    #[serde(rename = "narSize")]
    nar_size: u64,
    #[serde(rename = "closureSize")]
    closure_size: u64,
}

#[derive(Debug, Clone)]
pub struct PathInfo {
    pub path: StorePath,
    pub ca: Option<String>,
    pub deriver: Option<String>,
    pub nar_hash: String,
    pub nar_size: u64,
    pub closure_size: u64,
}

impl PathInfo {
    fn new(p: StorePath, inner: InnerPathInfo) -> Self {
        Self {
            path: p,
            ca: inner.ca,
            deriver: inner.deriver,
            nar_hash: inner.nar_hash,
            nar_size: inner.nar_size,
            closure_size: inner.closure_size,
        }
    }
}

fn extract_path_info(
    body: &[u8],
    path: StorePath,
    full_path: &str,
) -> Result<Option<PathInfo>, crate::Error> {
    let mut infos = serde_json::from_slice::<AHashMap<String, Option<InnerPathInfo>>>(body)?;
    Ok(infos
        .remove(full_path)
        .and_then(|i| i.map(|i| PathInfo::new(path, i))))
}

// TODO: evaluate if we need to limit memory here
#[cached(
    ty = "UnboundCache<String, Option<PathInfo>>",
    create = "{ UnboundCache::with_capacity(50) }",
    result = true,
    convert = r#"{ format!("{}", path) }"#
)]
#[tracing::instrument(fields(%path), err)]
pub async fn query_path_info(path: &StorePath) -> Result<Option<PathInfo>, crate::Error> {
    let full_path = path.get_full_path();
    let cmd = &tokio::process::Command::new("nix")
        .args([
            "path-info",
            "--json",
            "--size",
            "--closure-size",
            "--offline",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "",
            &full_path,
        ])
        .output()
        .await?;
    if cmd.status.success() {
        extract_path_info(&cmd.stdout, path.to_owned(), &full_path)
    } else {
        Ok(None)
    }
}

pub async fn clear_query_path_cache() {
    let mut cache = QUERY_PATH_INFO.lock().await;
    cache.cache_clear();
}

#[tracing::instrument(err)]
pub async fn query_path_infos(
    paths: &[&StorePath],
) -> Result<AHashMap<StorePath, PathInfo>, crate::Error> {
    let mut res = AHashMap::new();
    for p in paths {
        if let Some(info) = query_path_info(p).await? {
            res.insert((*p).to_owned(), info);
        }
    }
    Ok(res)
}

#[tracing::instrument(skip(path))]
pub async fn check_if_storepath_exists_using_pathinfo(path: &StorePath) -> bool {
    tokio::process::Command::new("nix")
        .args([
            "path-info",
            "--offline",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "",
            &path.get_full_path(),
        ])
        .output()
        .await
        .map(|cmd| cmd.status.success())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::extract_path_info;
    use crate::StorePath;

    #[test]
    fn can_extract_path_info() {
        let path = StorePath::new(
            "9hdbfg09ra0r8x2k501hqmhfaav4z8vx-nixos-system-helsinki-laptop12-25.05.drv",
        );
        let body = r#"{"/nix/store/9hdbfg09ra0r8x2k501hqmhfaav4z8vx-nixos-system-helsinki-laptop12-25.05.drv":{"ca":"text:sha256:07416qdchgddgfm02v3g01z7n63rmhjmga0glv3wf4nly3sw6y5a","closureSize":59548800,"deriver":null,"narHash":"sha256-72ema2mC9vU4ZmnbfHUrKe/jNp7P8MqX7rjgZ7nRR+w=","narSize":29240,"references":["/nix/store/0wy8117gx1hbdv85x2xq1vf12nlagan4-bash-interactive-5.2p37.drv","/nix/store/1ls88bhw8nh2chv4n4vlr1bcfz2h98kr-initrd-linux-6.12.36.drv","/nix/store/21ikdi17pcw3yj5d7g9890zkc0ciq8gi-gnugrep-3.11.drv","/nix/store/28nc4sz6frcy48imc1x6pfpa0yid5s5p-system-path.drv","/nix/store/2iglfxfaz8m3m0kcpgg4z5bk3xni0icq-switch-to-configuration-0.1.0.drv","/nix/store/316iziz4qcz2irjm5hcdiw4w49y7haa2-mounts.sh.drv","/nix/store/3k0iinqjfm948c2jwq4i8dq4gf1b8ibc-shadow-4.17.4.drv","/nix/store/3r7c5xdr9rv0lah8b52qvmz5awgvwzfi-coreutils-9.7.drv","/nix/store/3x86dl5ckqcc9kaanh743j7z1pwc8mfk-check-sshd-config.drv","/nix/store/41v0s5zxpwvvld02zvksy5d41k82f3aq-apparmor-profiles.drv","/nix/store/4981gs7vhp5jfvgd0drhvhn9py4wxfjs-append-initrd-secrets.drv","/nix/store/5c0vxsgk53d2fkzzrmad5iiwwckwjrqj-linux-6.12.36-modules.drv","/nix/store/5rfv6dd2ifk79mpa5jgjsplbkywzkbl1-perl-5.40.0-env.drv","/nix/store/7a4vkpyn0gzqdpk5hig0d7xcmvpw5j5a-bootinstall.drv","/nix/store/7sb1nkpf82nb5kj7qc4bbqkwj1l1mdv9-update-users-groups.pl","/nix/store/7shi1b9q4w2vki02fhp4a68xi8kl54g3-users-groups.json.drv","/nix/store/88hj17cdv53q45l27pajwalq7rhjl900-jq-1.7.1.drv","/nix/store/898n2qpxvgjpa6g0f2llpqp9vijdi6s2-kmod-31.drv","/nix/store/8b38dqrnwgg54vlk902nds78c498kclk-firmware.drv","/nix/store/8rzhs4yakwcb79q60im9bsr4y8gfi57g-linux-6.12.36.drv","/nix/store/arv0hndjkbwaiaxid23zidnb2zgkk77n-findutils-4.10.0.drv","/nix/store/d29qp3vda29w0y3rb00w7kngd80wahaj-manifest.json.drv","/nix/store/dbm4cjxds31vvr93bas4xvzn0p44q2ca-util-linux-2.41.drv","/nix/store/fqvanslnj1dj1ghr27kv35hnmc4vwhbs-getent-glibc-2.40-66.drv","/nix/store/gzacvsimj28rf8bs4rzshn5jpfnk9ack-pre-switch-checks.drv","/nix/store/hmzhbafafpl4g4mpm9hyjay0d786813k-ensure-all-wrappers-paths-exist.drv","/nix/store/ipd0army9h0v3fihrq55sq09qx6qc8dz-perl-5.40.0-env.drv","/nix/store/m04rpizy37lga67dddjw775iv1lwnbbc-make-shell-wrapper-hook.drv","/nix/store/mhxn5kwnri3z9hdzi3x0980id65p0icn-lib.sh","/nix/store/mqcgp7n2zvp65b9779zikgk02pm8inf0-stdenv-linux.drv","/nix/store/nl6v2f04l8mbmdvjg6gyrxyc89k503p4-boot.json.drv","/nix/store/p5hlsh9vanx5044ydkfip94sx0jg3c0i-systemd-257.6.drv","/nix/store/pb07q91z50krllm9vwxg7i24sv0plnlp-apparmor-abstractions.drv","/nix/store/pvp45ipdkv7xgk045d4646xnkn963fk3-etc.drv","/nix/store/rg5rf512szdxmnj9qal3wfdnpfsx38qi-setup-etc.pl","/nix/store/rwp1mbjfs320qnbf664jfj2mjwma9ldi-glibc-locales-2.40-66.drv","/nix/store/shkw4qm9qcw5sc5n1k5jznc83ny02r39-default-builder.sh","/nix/store/v9hj8b4z1pgj4ns8krnfpkbn1sj2bs39-apparmor-parser-4.1.1.drv","/nix/store/vj1c3wf9c11a0qs6p3ymfvrnsdgsdcbq-source-stdenv.sh","/nix/store/vzlq80x62454hshwysgh9jmdrcj5y10c-stage-2-init.sh.drv","/nix/store/xmx42ia0vx5777ch02629sr9rx18p51r-bash-5.2p37.drv","/nix/store/xryy5yhfa88adlz243kz8jzf9pdmrcjm-perl-5.40.0-env.drv","/nix/store/xvmcd6a5rrfpy38mdpk35gbwwkhy8fff-xkb-validated.drv","/nix/store/xw21z60ql9rfl41n17i7nh4l2sy48li5-net-tools-2.10.drv","/nix/store/xx2gyh45aiva41hlxq2i4jqgkvs7q937-glibc-2.40-66.drv","/nix/store/ym56xr6nlw69vmh3zvgydmhrxp0sy442-apparmor_unload.drv","/nix/store/yy2zvvzvg7675zgi0b3c2mqd03flzbhj-sops-install-secrets-0.0.1.drv","/nix/store/z0351vkic9cdh8qbdw5146gb9gbdfgwx-apparmor-profiles-4.1.1.drv"],"registrationTime":1751964968,"signatures":[],"ultimate":false}}"#;
        let info = extract_path_info(body.as_bytes(), path.clone(), &path.get_full_path())
            .unwrap()
            .unwrap();
        assert!(info.deriver.is_none());
        assert_eq!(
            info.ca,
            Some("text:sha256:07416qdchgddgfm02v3g01z7n63rmhjmga0glv3wf4nly3sw6y5a".into())
        );
        assert_eq!(
            info.nar_hash,
            "sha256-72ema2mC9vU4ZmnbfHUrKe/jNp7P8MqX7rjgZ7nRR+w=",
        );
        assert_eq!(info.nar_size, 29240);
        assert_eq!(info.closure_size, 59548800);
    }

    #[test]
    fn can_handle_null() {
        let path = StorePath::new(
            "9hdbfg09ra0r8x2k501hqmhfaav4z8vx-nixos-system-helsinki-laptop12-25.05.drv",
        );
        let body = r#"{"/nix/store/9hdbfg09ra0r8x2k501hqmhfaav4z8vx-nixos-system-helsinki-laptop12-25.05.drv":null}"#;
        let info = extract_path_info(body.as_bytes(), path.clone(), &path.get_full_path()).unwrap();
        assert!(info.is_none());
    }
}
