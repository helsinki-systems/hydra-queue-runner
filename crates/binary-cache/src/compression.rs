use async_compression::{
    Level,
    tokio::bufread::{BrotliEncoder, BzEncoder, XzEncoder, ZstdEncoder},
};

pub type CompressorFn<C> =
    Box<dyn FnOnce(C) -> Box<dyn tokio::io::AsyncRead + Unpin + Send> + Send>;

#[derive(Debug, Clone, Copy)]
pub enum Compression {
    None,
    Xz,
    Bzip2,
    Brotli,
    Zstd,
}

impl Compression {
    #[must_use]
    pub fn ext(self) -> &'static str {
        match self {
            Compression::None => "nar",
            Compression::Xz => "nar.xz",
            Compression::Bzip2 => "nar.bz2",
            Compression::Brotli => "nar.br",
            Compression::Zstd => "nar.zst",
        }
    }

    #[must_use]
    pub fn content_type(self) -> &'static str {
        "application/x-nix-nar"
    }

    #[must_use]
    pub fn content_encoding(self) -> &'static str {
        ""
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Compression::None => "none",
            Compression::Xz => "xz",
            Compression::Bzip2 => "bz2",
            Compression::Brotli => "br",
            Compression::Zstd => "zstd",
        }
    }

    #[must_use]
    pub fn get_compression_fn<C: tokio::io::AsyncBufRead + Unpin + Send + 'static>(
        self,
        level: Level,
        parallel: bool,
    ) -> CompressorFn<C> {
        match self {
            Compression::None => Box::new(|c| Box::new(c)),
            Compression::Xz => {
                if parallel && let Some(cores) = std::num::NonZero::new(4) {
                    Box::new(move |s| Box::new(XzEncoder::parallel(s, level, cores)))
                } else {
                    Box::new(move |s| Box::new(XzEncoder::with_quality(s, level)))
                }
            }
            Compression::Bzip2 => Box::new(move |s| Box::new(BzEncoder::with_quality(s, level))),
            Compression::Brotli => {
                Box::new(move |s| Box::new(BrotliEncoder::with_quality(s, level)))
            }
            Compression::Zstd => Box::new(move |s| Box::new(ZstdEncoder::with_quality(s, level))),
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Compression::None),
            "xz" => Ok(Compression::Xz),
            "bz2" => Ok(Compression::Bzip2),
            "br" => Ok(Compression::Brotli),
            "zstd" | "zst" => Ok(Compression::Zstd),
            o => Err(o.to_string()),
        }
    }
}
