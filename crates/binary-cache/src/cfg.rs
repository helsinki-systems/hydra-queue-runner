use crate::Compression;

const MIN_PRESIGNED_URL_EXPIRY_SECS: u64 = 60;
const MAX_PRESIGNED_URL_EXPIRY_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct S3CacheConfig {
    pub client_config: S3ClientConfig,

    pub compression: Compression,
    pub write_nar_listing: bool,
    pub write_debug_info: bool,
    pub secret_key_files: Vec<std::path::PathBuf>,
    pub parallel_compression: bool,
    pub compression_level: Option<i32>,

    pub narinfo_compression: Compression,
    pub ls_compression: Compression,
    pub log_compression: Compression,
    pub buffer_size: usize,

    pub presigned_url_expiry: std::time::Duration,
}

impl S3CacheConfig {
    #[must_use]
    pub fn new(client_config: S3ClientConfig) -> Self {
        Self {
            client_config,
            compression: Compression::Xz,
            write_nar_listing: false,
            write_debug_info: false,
            secret_key_files: Vec::default(),
            parallel_compression: false,
            compression_level: Option::default(),
            narinfo_compression: Compression::None,
            ls_compression: Compression::None,
            log_compression: Compression::None,
            buffer_size: 8 * 1024 * 1024,
            presigned_url_expiry: std::time::Duration::from_secs(3600),
        }
    }

    #[must_use]
    pub fn with_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_write_nar_listing(mut self, write_nar_listing: Option<&str>) -> Self {
        if let Some(write_nar_listing) = write_nar_listing {
            let s = write_nar_listing.trim().to_ascii_lowercase();
            self.write_nar_listing = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn with_write_debug_info(mut self, write_debug_info: Option<&str>) -> Self {
        if let Some(write_debug_info) = write_debug_info {
            let s = write_debug_info.trim().to_ascii_lowercase();
            self.write_debug_info = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn add_secret_key_files(mut self, secret_keys: &[std::path::PathBuf]) -> Self {
        self.secret_key_files.extend_from_slice(secret_keys);
        self
    }

    #[must_use]
    pub fn with_parallel_compression(mut self, parallel_compression: Option<&str>) -> Self {
        if let Some(parallel_compression) = parallel_compression {
            let s = parallel_compression.trim().to_ascii_lowercase();
            self.parallel_compression = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn with_compression_level(mut self, compression_level: Option<i32>) -> Self {
        if let Some(compression_level) = compression_level {
            self.compression_level = Some(compression_level);
        }
        self
    }

    #[must_use]
    pub fn with_narinfo_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.narinfo_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_ls_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.ls_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_log_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.log_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_buffer_size(mut self, buffer_size: Option<usize>) -> Self {
        if let Some(buffer_size) = buffer_size {
            self.buffer_size = buffer_size;
        }
        self
    }

    pub fn with_presigned_url_expiry(
        mut self,
        expiry_secs: Option<u64>,
    ) -> Result<Self, UrlParseError> {
        if let Some(expiry_secs) = expiry_secs {
            if !(MIN_PRESIGNED_URL_EXPIRY_SECS..=MAX_PRESIGNED_URL_EXPIRY_SECS)
                .contains(&expiry_secs)
            {
                return Err(UrlParseError::InvalidPresignedUrlExpiry(
                    expiry_secs,
                    MIN_PRESIGNED_URL_EXPIRY_SECS,
                    MAX_PRESIGNED_URL_EXPIRY_SECS,
                ));
            }
            self.presigned_url_expiry = std::time::Duration::from_secs(expiry_secs);
        }
        Ok(self)
    }

    pub(crate) fn get_compression_level(&self) -> async_compression::Level {
        if let Some(l) = self.compression_level {
            async_compression::Level::Precise(l)
        } else {
            async_compression::Level::Default
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlParseError {
    #[error("Uri parse error: {0}")]
    UriParseError(#[from] url::ParseError),
    #[error("Int parse error: {0}")]
    IntParseError(#[from] std::num::ParseIntError),
    #[error("Invalid S3Scheme: {0}")]
    S3SchemeParseError(String),
    #[error("Invalid Compression: {0}")]
    CompressionParseError(String),
    #[error("Bad schema: {0}")]
    BadSchema(String),
    #[error("Bucket not defined")]
    NoBucket,
    #[error("Invalid presigned URL expiry: {0}. Must be between {1} and {2} seconds")]
    InvalidPresignedUrlExpiry(u64, u64, u64),
}

impl std::str::FromStr for S3CacheConfig {
    type Err = UrlParseError;

    #[allow(clippy::too_many_lines)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri = url::Url::parse(&s.trim().to_ascii_lowercase())?;
        if uri.scheme() != "s3" {
            return Err(UrlParseError::BadSchema(uri.scheme().to_owned()));
        }
        let bucket = uri.authority();
        if bucket.is_empty() {
            return Err(UrlParseError::NoBucket);
        }
        let query = uri
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        let cfg = S3ClientConfig::new(bucket.to_owned())
            .with_region(query.get("region").map(std::string::String::as_str))
            .with_scheme(
                query
                    .get("scheme")
                    .map(|x| x.parse::<S3Scheme>())
                    .transpose()
                    .map_err(UrlParseError::S3SchemeParseError)?,
            )
            .with_endpoint(query.get("endpoint").map(std::string::String::as_str))
            .with_profile(query.get("profile").map(std::string::String::as_str));

        S3CacheConfig::new(cfg)
            .with_compression(
                query
                    .get("compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_write_nar_listing(
                query
                    .get("write-nar-listing")
                    .map(std::string::String::as_str),
            )
            .with_write_debug_info(
                query
                    .get("write-debug-info")
                    .map(std::string::String::as_str),
            )
            .add_secret_key_files(
                &query
                    .get("secret-key")
                    .map(|s| if s.is_empty() { vec![] } else { vec![s.into()] })
                    .unwrap_or_default(),
            )
            .add_secret_key_files(
                &query
                    .get("secret-keys")
                    .map(|s| {
                        s.split(',')
                            .filter(|s| !s.is_empty())
                            .map(Into::into)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
            .with_parallel_compression(
                query
                    .get("parallel-compression")
                    .map(std::string::String::as_str),
            )
            .with_compression_level(
                query
                    .get("compression-level")
                    .map(|x| x.parse::<i32>())
                    .transpose()?,
            )
            .with_narinfo_compression(
                query
                    .get("narinfo-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_ls_compression(
                query
                    .get("ls-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_log_compression(
                query
                    .get("log-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_buffer_size(
                query
                    .get("buffer-size")
                    .map(|x| x.parse::<usize>())
                    .transpose()?,
            )
            .with_presigned_url_expiry(
                query
                    .get("presigned-url-expiry")
                    .map(|x| x.parse::<u64>())
                    .transpose()?,
            )
    }
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq)]
pub enum S3Scheme {
    HTTP,
    #[default]
    HTTPS,
}

impl std::str::FromStr for S3Scheme {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "http" => Ok(S3Scheme::HTTP),
            "https" => Ok(S3Scheme::HTTPS),
            v => Err(v.to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct S3ClientConfig {
    pub region: String,
    pub scheme: S3Scheme,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub profile: Option<String>,
    pub(crate) credentials: Option<S3CredentialsConfig>,
}

impl S3ClientConfig {
    #[must_use]
    pub fn new(bucket: String) -> Self {
        Self {
            region: "us-east-1".into(),
            scheme: S3Scheme::default(),
            endpoint: None,
            bucket,
            profile: None,
            credentials: None,
        }
    }

    #[must_use]
    pub fn with_region(mut self, region: Option<&str>) -> Self {
        if let Some(region) = region {
            self.region = region.into();
        }
        self
    }

    #[must_use]
    pub fn with_scheme(mut self, scheme: Option<S3Scheme>) -> Self {
        if let Some(scheme) = scheme {
            self.scheme = scheme;
        }
        self
    }

    #[must_use]
    pub fn with_endpoint(mut self, endpoint: Option<&str>) -> Self {
        self.endpoint = endpoint.map(ToOwned::to_owned);
        self
    }

    #[must_use]
    pub fn with_profile(mut self, profile: Option<&str>) -> Self {
        self.profile = profile.map(ToOwned::to_owned);
        self
    }

    #[must_use]
    pub fn with_credentials(mut self, credentials: Option<S3CredentialsConfig>) -> Self {
        self.credentials = credentials;
        self
    }
}

#[derive(Debug, Clone)]
pub struct S3CredentialsConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigReadError {
    #[error("Env var not found: {0}")]
    EnvVarNotFound(#[from] std::env::VarError),
    #[error("Read error: {0}")]
    ReadError(String),
    #[error("Profile missing: {0}")]
    ProfileMissing(String),
    #[error("Value missing: {0}")]
    ValueMissing(&'static str),
}

pub(crate) fn read_aws_credentials_file(
    profile: &str,
) -> Result<(String, String), ConfigReadError> {
    let home_dir = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))?;
    let credentials_path = format!("{home_dir}/.aws/credentials");

    let mut config = configparser::ini::Ini::new();
    let config_map = config
        .load(&credentials_path)
        .map_err(ConfigReadError::ReadError)?;
    parse_aws_credentials_file(&config_map, profile)
}

fn parse_aws_credentials_file(
    config_map: &std::collections::HashMap<
        String,
        std::collections::HashMap<String, Option<String>>,
    >,
    profile: &str,
) -> Result<(String, String), ConfigReadError> {
    let profile_map = if let Some(profile_map) = config_map.get(profile) {
        profile_map
    } else if let Some(profile_map) = config_map.get(&format!("profile {profile}")) {
        profile_map
    } else {
        let mut r_section_map = None;
        for (section_name, section_map) in config_map {
            let trimmed_section = section_name.trim();
            if trimmed_section == profile || trimmed_section == format!("profile {profile}") {
                r_section_map = Some(section_map);
                break;
            }
        }
        if let Some(section_map) = r_section_map {
            section_map
        } else {
            return Err(ConfigReadError::ProfileMissing(profile.into()));
        }
    };

    let access_key = profile_map
        .get("aws_access_key_id")
        .and_then(ToOwned::to_owned)
        .ok_or(ConfigReadError::ValueMissing("aws_access_key_id"))?
        .clone();
    let secret_key = profile_map
        .get("aws_secret_access_key")
        .and_then(ToOwned::to_owned)
        .ok_or(ConfigReadError::ValueMissing("aws_secret_access_key"))?
        .clone();

    Ok((access_key, secret_key))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{ConfigReadError, S3CacheConfig, UrlParseError, parse_aws_credentials_file};
    use std::str::FromStr as _;

    #[test]
    fn test_parsing_default_profile_works() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
# AWS credentials file format:
# ~/.aws/credentials
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[production]
aws_access_key_id = AKIAI44QH8DHBEXAMPLE
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY"
                    .into(),
            )
            .unwrap();

        let (access_key, secret_key) = parse_aws_credentials_file(&config_map, "default").unwrap();
        assert_eq!(access_key, "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(secret_key, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
    }

    #[test]
    fn test_parsing_profile_with_spaces_and_comments() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
# This is a comment
# AWS credentials file with various formatting
[default]
# Another comment before the key
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

# Profile with spaces in name
[profile test]
aws_access_key_id = AKIAI44QH8DHBEXAMPLE
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY

# Profile with extra whitespace
[   staging   ]
aws_access_key_id = AKIAI44QH8DHBSTAGING
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbSTAGINGKEY"
                    .into(),
            )
            .unwrap();

        println!(
            "Available profiles: {:?}",
            config_map.keys().collect::<Vec<_>>()
        );

        let (access_key, secret_key) = parse_aws_credentials_file(&config_map, "default").unwrap();
        assert_eq!(access_key, "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(secret_key, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");

        let (access_key, secret_key) = parse_aws_credentials_file(&config_map, "test").unwrap();
        assert_eq!(access_key, "AKIAI44QH8DHBEXAMPLE");
        assert_eq!(secret_key, "je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY");

        let (access_key, secret_key) = parse_aws_credentials_file(&config_map, "staging").unwrap();
        assert_eq!(access_key, "AKIAI44QH8DHBSTAGING");
        assert_eq!(secret_key, "je7MtGbClwBF/2Zp9Utk/h3yCo8nvbSTAGINGKEY");
    }

    #[test]
    fn test_missing_profile_returns_error() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
                    .into(),
            )
            .unwrap();

        let result = parse_aws_credentials_file(&config_map, "nonexistent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigReadError::ProfileMissing(_)
        ));
    }

    #[test]
    fn test_missing_access_key_returns_error() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
[default]
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
                    .into(),
            )
            .unwrap();

        let result = parse_aws_credentials_file(&config_map, "default");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigReadError::ValueMissing("aws_access_key_id")
        ));
    }

    #[test]
    fn test_missing_secret_key_returns_error() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE"
                    .into(),
            )
            .unwrap();

        let result = parse_aws_credentials_file(&config_map, "default");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigReadError::ValueMissing("aws_secret_access_key")
        ));
    }

    #[test]
    fn test_empty_credentials_file_returns_error() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config.read(String::new()).unwrap();

        let result = parse_aws_credentials_file(&config_map, "default");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigReadError::ProfileMissing(_)
        ));
    }

    #[test]
    fn test_profile_with_special_characters() {
        let mut config = configparser::ini::Ini::new();
        let config_map = config
            .read(
                r"
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[my-test_profile]
aws_access_key_id = AKIAI44QH8DHBTEST
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbTESTKEY

[profile_123]
aws_access_key_id = AKIAI44QH8DHB123
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvb123KEY"
                    .into(),
            )
            .unwrap();

        let (access_key, secret_key) =
            parse_aws_credentials_file(&config_map, "my-test_profile").unwrap();
        assert_eq!(access_key, "AKIAI44QH8DHBTEST");
        assert_eq!(secret_key, "je7MtGbClwBF/2Zp9Utk/h3yCo8nvbTESTKEY");

        let (access_key, secret_key) =
            parse_aws_credentials_file(&config_map, "profile_123").unwrap();
        assert_eq!(access_key, "AKIAI44QH8DHB123");
        assert_eq!(secret_key, "je7MtGbClwBF/2Zp9Utk/h3yCo8nvb123KEY");
    }

    #[test]
    fn test_presigned_url_expiry_validation() {
        // Test valid expiry times
        let valid_cases = vec!["60", "3600", "86400"]; // 1min, 1hr, 1day, 7days
        for expiry in valid_cases {
            let config_str = format!("s3://test-bucket?presigned-url-expiry={expiry}");
            let result = S3CacheConfig::from_str(&config_str);
            assert!(result.is_ok(), "Should accept expiry: {expiry}");
        }

        // Test invalid expiry times
        let invalid_cases = vec!["0", "30", "604801"]; // too small, too small, too large
        for expiry in invalid_cases {
            let config_str = format!("s3://test-bucket?presigned-url-expiry={expiry}");
            let result = S3CacheConfig::from_str(&config_str);
            assert!(result.is_err(), "Should reject expiry: {expiry}");
            if let Err(UrlParseError::InvalidPresignedUrlExpiry(value, min, max)) = result {
                assert_eq!(value, expiry.parse::<u64>().unwrap());
                assert_eq!(min, 60);
                assert_eq!(max, 86_400);
            } else {
                panic!("Expected InvalidPresignedUrlExpiry error");
            }
        }
    }
}
