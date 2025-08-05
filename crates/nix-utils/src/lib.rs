mod drv;
mod nix_support;
mod pathinfo;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("serde json error: `{0}`")]
    SerdeJson(#[from] serde_json::Error),

    #[error("std io error: `{0}`")]
    Io(#[from] std::io::Error),

    #[error("tokio join error: `{0}`")]
    TokioJoin(#[from] tokio::task::JoinError),

    #[error("utf8 error: `{0}`")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("Failed to get tokio stdout stream")]
    Stream,

    #[error("regex error: `{0}`")]
    Regex(#[from] regex::Error),

    #[error("Command failed with `{0}`")]
    Exit(std::process::ExitStatus),

    #[error("Exception was thrown `{0}`")]
    Exception(#[from] cxx::Exception),
}

pub use drv::{
    BuildOptions, Derivation, Output as DerivationOutput, get_outputs_for_drv,
    get_outputs_for_drvs, query_drv, query_drvs, query_missing_outputs, realise_drv, realise_drvs,
    topo_sort_drvs,
};
pub use nix_support::{BuildMetric, BuildProduct, NixSupport, parse_nix_support_from_outputs};
pub use pathinfo::{
    PathInfo, check_if_storepath_exists_using_pathinfo, clear_query_path_cache, query_path_info,
    query_path_infos,
};

pub const HASH_LEN: usize = 32;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorePath {
    base_name: String,
}

impl StorePath {
    pub fn new(p: &str) -> Self {
        if let Some(postfix) = p.strip_prefix("/nix/store/") {
            debug_assert!(postfix.len() > HASH_LEN + 1);
            Self {
                base_name: postfix.to_string(),
            }
        } else {
            debug_assert!(p.len() > HASH_LEN + 1);
            Self {
                base_name: p.to_string(),
            }
        }
    }

    pub fn base_name(&self) -> &str {
        &self.base_name
    }

    pub fn name(&self) -> &str {
        &self.base_name[HASH_LEN + 1..]
    }

    pub fn hash_part(&self) -> &str {
        &self.base_name[..HASH_LEN]
    }

    pub fn is_drv(&self) -> bool {
        self.base_name.ends_with(".drv")
    }

    pub fn get_full_path(&self) -> String {
        format!("/nix/store/{}", self.base_name)
    }
}
impl serde::Serialize for StorePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.base_name())
    }
}

impl std::fmt::Display for StorePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self.base_name)
    }
}

#[tracing::instrument(skip(path))]
pub async fn check_if_storepath_exists(path: &StorePath) -> bool {
    tokio::fs::try_exists(&path.get_full_path())
        .await
        .unwrap_or_default()
}

pub fn validate_statuscode(status: std::process::ExitStatus) -> Result<(), Error> {
    if status.success() {
        Ok(())
    } else {
        Err(Error::Exit(status))
    }
}

pub fn add_root(root_dir: &std::path::Path, store_path: &StorePath) {
    let path = root_dir.join(store_path.base_name());
    // force create symlink
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    if !path.exists() {
        let _ = std::os::unix::fs::symlink(store_path.get_full_path(), path);
    }
}

#[cxx::bridge(namespace = "nix_utils")]
mod ffi {
    #[derive(Debug)]
    struct S3Stats {
        put: u64,
        put_bytes: u64,
        put_time_ms: u64,
        get: u64,
        get_bytes: u64,
        get_time_ms: u64,
        head: u64,
    }

    unsafe extern "C++" {
        include!("nix-utils/include/nix.h");

        type StoreWrapper;

        fn init(uri: &str) -> SharedPtr<StoreWrapper>;

        fn get_nix_prefix() -> String;
        fn get_store_dir() -> String;
        fn get_log_dir() -> String;
        fn get_state_dir() -> String;
        fn get_this_system() -> String;
        fn get_extra_platforms() -> Vec<String>;
        fn get_system_features() -> Vec<String>;
        fn get_use_cgroups() -> bool;
        fn set_verbosity(level: i32);

        fn is_valid_path(store: &StoreWrapper, path: &str) -> Result<bool>;
        fn compute_fs_closure(
            store: &StoreWrapper,
            path: &str,
            flip_direction: bool,
            include_outputs: bool,
            include_derivers: bool,
        ) -> Result<Vec<String>>;
        fn upsert_file(store: &StoreWrapper, path: &str, data: &str, mime_type: &str)
        -> Result<()>;
        fn get_s3_stats(store: &StoreWrapper) -> Result<S3Stats>;
        fn copy_paths(
            src_store: &StoreWrapper,
            dst_store: &StoreWrapper,
            paths: &[&str],
            repair: bool,
            check_sigs: bool,
            substitute: bool,
        ) -> Result<()>;

        fn import_paths(
            store: &StoreWrapper,
            check_sigs: bool,
            runtime: usize,
            callback: unsafe extern "C" fn(
                data: &mut [u8],
                runtime: usize,
                user_data: usize,
            ) -> usize,
            user_data: usize,
        ) -> Result<()>;
        fn import_paths_with_fd(store: &StoreWrapper, check_sigs: bool, fd: i32) -> Result<()>;
        fn export_paths(
            store: &StoreWrapper,
            paths: &[&str],
            callback: unsafe extern "C" fn(data: &[u8], user_data: usize) -> bool,
            user_data: usize,
        ) -> Result<()>;
    }
}

#[inline]
#[must_use]
pub fn get_nix_prefix() -> String {
    ffi::get_nix_prefix()
}

#[inline]
#[must_use]
pub fn get_store_dir() -> String {
    ffi::get_store_dir()
}

#[inline]
#[must_use]
pub fn get_log_dir() -> String {
    ffi::get_log_dir()
}

#[inline]
#[must_use]
pub fn get_state_dir() -> String {
    ffi::get_state_dir()
}

#[inline]
#[must_use]
pub fn get_this_system() -> String {
    ffi::get_this_system()
}

#[inline]
#[must_use]
pub fn get_extra_platforms() -> Vec<String> {
    ffi::get_extra_platforms()
}

#[inline]
#[must_use]
pub fn get_system_features() -> Vec<String> {
    ffi::get_system_features()
}

#[inline]
#[must_use]
pub fn get_use_cgroups() -> bool {
    ffi::get_use_cgroups()
}

#[inline]
/// Set the loglevel.
pub fn set_verbosity(level: i32) {
    ffi::set_verbosity(level);
}

#[inline]
pub fn copy_paths(
    src: &BaseStoreImpl,
    dst: &BaseStoreImpl,
    paths: &[StorePath],
    repair: bool,
    check_sigs: bool,
    substitute: bool,
) -> Result<(), cxx::Exception> {
    let paths = paths.iter().map(|v| v.get_full_path()).collect::<Vec<_>>();
    let slice = paths.iter().map(|v| v.as_str()).collect::<Vec<_>>();
    ffi::copy_paths(
        &src.wrapper,
        &dst.wrapper,
        &slice,
        repair,
        check_sigs,
        substitute,
    )
}

pub trait BaseStore {
    #[must_use]
    /// Check whether a path is valid.
    fn is_valid_path(&self, path: &str) -> bool;

    fn compute_fs_closure(
        &self,
        path: &str,
        flip_direction: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> Result<Vec<String>, cxx::Exception>;

    fn import_paths_with_cb<F>(&self, callback: F, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        F: FnMut(&tokio::runtime::Runtime, &mut [u8]) -> usize;

    /// Import paths from nar
    fn import_paths<S>(
        &self,
        stream: S,
        check_sigs: bool,
    ) -> impl std::future::Future<Output = Result<(), Error>>
    where
        S: tokio_stream::Stream<Item = Result<bytes::Bytes, std::io::Error>>
            + Send
            + Unpin
            + 'static;

    /// Import paths from nar
    fn import_paths_with_fd<Fd>(&self, fd: Fd, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        Fd: std::os::fd::AsFd + std::os::fd::AsRawFd;

    /// Export a store path in NAR format. The data is passed in chunks to callback
    fn export_paths<F>(&self, paths: &[StorePath], callback: F) -> Result<(), cxx::Exception>
    where
        F: FnMut(&[u8]) -> bool;
}

unsafe impl Send for crate::ffi::StoreWrapper {}
unsafe impl Sync for crate::ffi::StoreWrapper {}

#[derive(Clone)]
pub struct BaseStoreImpl {
    wrapper: cxx::SharedPtr<crate::ffi::StoreWrapper>,
}

impl BaseStoreImpl {
    fn new(store: cxx::SharedPtr<crate::ffi::StoreWrapper>) -> Self {
        Self { wrapper: store }
    }
}

fn import_paths_trampoline<F>(data: &mut [u8], runtime: usize, userdata: usize) -> usize
where
    F: FnMut(&tokio::runtime::Runtime, &mut [u8]) -> usize,
{
    let runtime = unsafe { *(runtime as *mut std::ffi::c_void).cast::<&tokio::runtime::Runtime>() };
    let closure = unsafe { &mut *(userdata as *mut std::ffi::c_void).cast::<F>() };
    closure(runtime, data)
}

fn export_paths_trampoline<F>(data: &[u8], userdata: usize) -> bool
where
    F: FnMut(&[u8]) -> bool,
{
    let closure = unsafe { &mut *(userdata as *mut std::ffi::c_void).cast::<F>() };
    closure(data)
}

impl BaseStore for BaseStoreImpl {
    #[inline]
    fn is_valid_path(&self, path: &str) -> bool {
        ffi::is_valid_path(&self.wrapper, path).unwrap_or(false)
    }

    #[inline]
    #[tracing::instrument(skip(self), err)]
    fn compute_fs_closure(
        &self,
        path: &str,
        flip_direction: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> Result<Vec<String>, cxx::Exception> {
        ffi::compute_fs_closure(
            &self.wrapper,
            path,
            flip_direction,
            include_outputs,
            include_derivers,
        )
    }

    #[inline]
    #[tracing::instrument(skip(self, callback), err)]
    fn import_paths_with_cb<F>(&self, callback: F, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        F: FnMut(&tokio::runtime::Runtime, &mut [u8]) -> usize,
    {
        let runtime = Box::pin(tokio::runtime::Runtime::new().unwrap());
        ffi::import_paths(
            &self.wrapper,
            check_sigs,
            std::ptr::addr_of!(runtime).cast::<std::ffi::c_void>() as usize,
            import_paths_trampoline::<F>,
            std::ptr::addr_of!(callback).cast::<std::ffi::c_void>() as usize,
        )
    }

    #[inline]
    #[tracing::instrument(skip(self, stream), err)]
    async fn import_paths<S>(&self, stream: S, check_sigs: bool) -> Result<(), Error>
    where
        S: tokio_stream::Stream<Item = Result<bytes::Bytes, std::io::Error>>
            + Send
            + Unpin
            + 'static,
    {
        use tokio::io::AsyncReadExt as _;

        let mut reader = tokio_util::io::StreamReader::new(stream);
        let callback = move |runtime: &tokio::runtime::Runtime, data: &mut [u8]| {
            runtime.block_on(async { reader.read(data).await.unwrap_or(0) })
        };
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.import_paths_with_cb(callback, check_sigs))
            .await??;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(skip(self, fd), err)]
    fn import_paths_with_fd<Fd>(&self, fd: Fd, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        Fd: std::os::fd::AsFd + std::os::fd::AsRawFd,
    {
        ffi::import_paths_with_fd(&self.wrapper, check_sigs, fd.as_raw_fd())
    }

    #[inline]
    #[tracing::instrument(skip(self, paths, callback), err)]
    fn export_paths<F>(&self, paths: &[StorePath], callback: F) -> Result<(), cxx::Exception>
    where
        F: FnMut(&[u8]) -> bool,
    {
        let paths = paths.iter().map(|v| v.get_full_path()).collect::<Vec<_>>();
        let slice = paths.iter().map(|v| v.as_str()).collect::<Vec<_>>();
        ffi::export_paths(
            &self.wrapper,
            &slice,
            export_paths_trampoline::<F>,
            std::ptr::addr_of!(callback).cast::<std::ffi::c_void>() as usize,
        )
    }
}

#[derive(Clone)]
pub struct LocalStore {
    base: BaseStoreImpl,
}

impl LocalStore {
    #[inline]
    /// Initialise a new store
    pub fn init() -> Self {
        Self {
            base: BaseStoreImpl::new(ffi::init("")),
        }
    }

    pub fn as_base_store(&self) -> &BaseStoreImpl {
        &self.base
    }
}

impl BaseStore for LocalStore {
    #[inline]
    fn is_valid_path(&self, path: &str) -> bool {
        self.base.is_valid_path(path)
    }

    #[inline]
    #[tracing::instrument(skip(self), err)]
    fn compute_fs_closure(
        &self,
        path: &str,
        flip_direction: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> Result<Vec<String>, cxx::Exception> {
        self.base
            .compute_fs_closure(path, flip_direction, include_outputs, include_derivers)
    }

    #[inline]
    #[tracing::instrument(skip(self, callback), err)]
    fn import_paths_with_cb<F>(&self, callback: F, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        F: FnMut(&tokio::runtime::Runtime, &mut [u8]) -> usize,
    {
        self.base.import_paths_with_cb::<F>(callback, check_sigs)
    }

    #[inline]
    #[tracing::instrument(skip(self, stream), err)]
    async fn import_paths<S>(&self, stream: S, check_sigs: bool) -> Result<(), Error>
    where
        S: tokio_stream::Stream<Item = Result<bytes::Bytes, std::io::Error>>
            + Send
            + Unpin
            + 'static,
    {
        self.base.import_paths::<S>(stream, check_sigs).await
    }

    #[inline]
    #[tracing::instrument(skip(self, fd), err)]
    fn import_paths_with_fd<Fd>(&self, fd: Fd, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        Fd: std::os::fd::AsFd + std::os::fd::AsRawFd,
    {
        self.base.import_paths_with_fd(fd, check_sigs)
    }

    #[inline]
    #[tracing::instrument(skip(self, paths, callback), err)]
    fn export_paths<F>(&self, paths: &[StorePath], callback: F) -> Result<(), cxx::Exception>
    where
        F: FnMut(&[u8]) -> bool,
    {
        self.base.export_paths(paths, callback)
    }
}

#[derive(Clone)]
pub struct RemoteStore {
    base: BaseStoreImpl,
}

impl RemoteStore {
    #[inline]
    /// Initialise a new store with uri
    pub fn init(uri: &str) -> Self {
        Self {
            base: BaseStoreImpl::new(ffi::init(uri)),
        }
    }

    pub fn as_base_store(&self) -> &BaseStoreImpl {
        &self.base
    }

    #[inline]
    pub fn upsert_file(
        &self,
        path: &str,
        data: &str,
        mime_type: &str,
    ) -> Result<(), cxx::Exception> {
        ffi::upsert_file(&self.base.wrapper, path, data, mime_type)
    }

    #[inline]
    pub fn get_s3_stats(&self) -> Result<crate::ffi::S3Stats, cxx::Exception> {
        ffi::get_s3_stats(&self.base.wrapper)
    }

    #[tracing::instrument(skip(self, outputs))]
    pub fn query_missing_remote_outputs(
        &self,
        outputs: Vec<DerivationOutput>,
    ) -> Vec<DerivationOutput> {
        outputs
            .into_iter()
            .filter_map(|o| {
                let Some(path) = &o.path else {
                    return None;
                };
                if !self.is_valid_path(&path.get_full_path()) {
                    Some(o)
                } else {
                    None
                }
            })
            .collect()
    }
}

impl BaseStore for RemoteStore {
    #[inline]
    fn is_valid_path(&self, path: &str) -> bool {
        self.base.is_valid_path(path)
    }

    #[inline]
    #[tracing::instrument(skip(self), err)]
    fn compute_fs_closure(
        &self,
        path: &str,
        flip_direction: bool,
        include_outputs: bool,
        include_derivers: bool,
    ) -> Result<Vec<String>, cxx::Exception> {
        self.base
            .compute_fs_closure(path, flip_direction, include_outputs, include_derivers)
    }

    #[inline]
    #[tracing::instrument(skip(self, callback), err)]
    fn import_paths_with_cb<F>(&self, callback: F, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        F: FnMut(&tokio::runtime::Runtime, &mut [u8]) -> usize,
    {
        self.base.import_paths_with_cb::<F>(callback, check_sigs)
    }

    #[inline]
    #[tracing::instrument(skip(self, stream), err)]
    async fn import_paths<S>(&self, stream: S, check_sigs: bool) -> Result<(), Error>
    where
        S: tokio_stream::Stream<Item = Result<bytes::Bytes, std::io::Error>>
            + Send
            + Unpin
            + 'static,
    {
        self.base.import_paths::<S>(stream, check_sigs).await
    }

    #[inline]
    #[tracing::instrument(skip(self, fd), err)]
    fn import_paths_with_fd<Fd>(&self, fd: Fd, check_sigs: bool) -> Result<(), cxx::Exception>
    where
        Fd: std::os::fd::AsFd + std::os::fd::AsRawFd,
    {
        self.base.import_paths_with_fd(fd, check_sigs)
    }

    #[inline]
    #[tracing::instrument(skip(self, paths, callback), err)]
    fn export_paths<F>(&self, paths: &[StorePath], callback: F) -> Result<(), cxx::Exception>
    where
        F: FnMut(&[u8]) -> bool,
    {
        self.base.export_paths(paths, callback)
    }
}
