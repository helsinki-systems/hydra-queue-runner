#include "nix-utils/include/nix.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <nix/store/remote-store.hh>
#include <nix/store/store-api.hh>
#include <nix/store/store-open.hh>

#include <nix/main/shared.hh>
#include <nix/store/binary-cache-store.hh>
#include <nix/store/globals.hh>
#include <nix/store/s3-binary-cache-store.hh>

static std::atomic<bool> initializedNix = false;

#define STR(rstr) std::string(rstr.data(), rstr.length())

static inline rust::Vec<rust::String>
extract_path_set(const nix::Store &store, const nix::StorePathSet &set) {
  rust::Vec<rust::String> data;
  data.reserve(set.size());
  for (const nix::StorePath &path : set) {
    data.emplace_back(store.printStorePath(path));
  }
  return data;
}

namespace nix_utils {
StoreWrapper::StoreWrapper(nix::ref<nix::Store> _store) : _store(_store) {}

std::shared_ptr<StoreWrapper> init(rust::Str uri) {
  if (!initializedNix) {
    initializedNix = true;
    nix::initLibStore();
  }
  if (uri.empty()) {
    nix::ref<nix::Store> _store = nix::openStore();
    return std::make_shared<StoreWrapper>(_store);
  } else {
    nix::ref<nix::Store> _store = nix::openStore(STR(uri));
    return std::make_shared<StoreWrapper>(_store);
  }
}

rust::String get_nix_prefix() { return nix::settings.nixPrefix; }
rust::String get_store_dir() { return nix::settings.nixStore; }
rust::String get_log_dir() { return nix::settings.nixLogDir; }
rust::String get_state_dir() { return nix::settings.nixStateDir; }
rust::String get_this_system() { return nix::settings.thisSystem.get(); }
rust::Vec<rust::String> get_extra_platforms() {
  auto set = nix::settings.extraPlatforms.get();
  rust::Vec<rust::String> data;
  data.reserve(set.size());
  for (const auto &val : set) {
    data.emplace_back(val);
  }
  return data;
}
rust::Vec<rust::String> get_system_features() {
  auto set = nix::settings.systemFeatures.get();
  rust::Vec<rust::String> data;
  data.reserve(set.size());
  for (const auto &val : set) {
    data.emplace_back(val);
  }
  return data;
}
bool get_use_cgroups() {
#ifdef __linux__
  return nix::settings.useCgroups;
#endif
  return false;
}
void set_verbosity(int32_t level) { nix::verbosity = (nix::Verbosity)level; }

bool is_valid_path(const StoreWrapper &wrapper, rust::Str path) {
  auto store = wrapper._store;
  return store->isValidPath(store->parseStorePath(STR(path)));
}

rust::Vec<rust::String> compute_fs_closure(const StoreWrapper &wrapper,
                                           rust::Str path, bool flip_direction,
                                           bool include_outputs,
                                           bool include_derivers) {
  auto store = wrapper._store;
  nix::StorePathSet path_set;
  store->computeFSClosure(store->parseStorePath(STR(path)), path_set,
                          flip_direction, include_outputs, include_derivers);
  return extract_path_set(*store, path_set);
}

void upsert_file(const StoreWrapper &wrapper, rust::Str path, rust::Str data,
                 rust::Str mime_type) {
  auto store = wrapper._store.dynamic_pointer_cast<nix::BinaryCacheStore>();
  if (!store) {
    throw nix::Error("Not a binary chache store");
  }
  store->upsertFile(STR(path), STR(data), STR(mime_type));
}

S3Stats get_s3_stats(const StoreWrapper &wrapper) {
  auto store = wrapper._store;
  auto s3Store = dynamic_cast<nix::S3BinaryCacheStore *>(&*store);
  if (!s3Store) {
    throw nix::Error("Not a s3 binary chache store");
  }
  auto &stats = s3Store->getS3Stats();
  return S3Stats{
      stats.put.load(),  stats.putBytes.load(), stats.putTimeMs.load(),
      stats.get.load(),  stats.getBytes.load(), stats.getTimeMs.load(),
      stats.head.load(),
  };
}

void copy_paths(const StoreWrapper &src_store, const StoreWrapper &dst_store,
                rust::Slice<const rust::Str> paths, bool repair,
                bool check_sigs, bool substitute) {
  nix::StorePathSet path_set;
  for (auto &path : paths) {
    path_set.insert(src_store._store->parseStorePath(STR(path)));
  }
  nix::copyPaths(*src_store._store, *dst_store._store, path_set,
                 (nix::RepairFlag)repair, (nix::CheckSigsFlag)check_sigs,
                 (nix::SubstituteFlag)substitute);
}

void import_paths(
    const StoreWrapper &wrapper, bool check_sigs, size_t runtime,
    rust::Fn<size_t(rust::Slice<uint8_t>, long unsigned int, long unsigned int)>
        callback,
    size_t user_data) {
  nix::LambdaSource source([=](char *out, size_t out_len) {
    auto data = rust::Slice<uint8_t>((uint8_t *)out, out_len);
    size_t ret = (*callback)(data, runtime, user_data);
    if (!ret) {
      throw nix::EndOfFile("End of stream reached");
    }
    return ret;
  });

  try {
    auto store = wrapper._store;
    store->importPaths(source, (nix::CheckSigsFlag)check_sigs);
  } catch (nix::EndOfFile &e) {
    // Intentionally do nothing. We're only using the exception as a
    // short-circuiting mechanism.
  }
}

void import_paths_with_fd(const StoreWrapper &wrapper, bool check_sigs,
                          int fd) {
  nix::FdSource source(fd);

  auto store = wrapper._store;
  store->importPaths(source, (nix::CheckSigsFlag)check_sigs);
}

class StopExport : public std::exception {
public:
  const char *what() { return "Stop exporting nar"; }
};

void export_paths(
    const StoreWrapper &wrapper, rust::Slice<const rust::Str> paths,
    rust::Fn<bool(rust::Slice<const uint8_t>, long unsigned int)> callback,
    size_t user_data) {
  nix::LambdaSink sink([=](std::string_view v) {
    auto data = rust::Slice<const uint8_t>((const uint8_t *)v.data(), v.size());
    bool ret = (*callback)(data, user_data);
    if (!ret) {
      throw StopExport();
    }
  });

  auto store = wrapper._store;
  nix::StorePathSet path_set;
  for (auto &path : paths) {
    path_set.insert(store->parseStorePath(STR(path)));
  }
  try {
    store->exportPaths(path_set, sink);
  } catch (StopExport &e) {
    // Intentionally do nothing. We're only using the exception as a
    // short-circuiting mechanism.
  }
}
} // namespace nix_utils
