#pragma once

#include "rust/cxx.h"
#include <memory>

#include "nix-utils/include/nix.h"
#include "nix/store/realisation.hh"

namespace nix_utils {
struct Realisation;

class InternalRealisation {
public:
  InternalRealisation(nix::ref<nix::Realisation> _realisation);

  rust::String as_json() const;
  Realisation to_rust(const nix_utils::StoreWrapper &wrapper) const;

  rust::String fingerprint() const;
  void sign(rust::Str secret_key);

  void write_to_disk_cache(const nix_utils::StoreWrapper &wrapper) const;

private:
  nix::ref<nix::Realisation> _realisation;
};
} // namespace nix_utils

#include "nix-utils/src/realisation.rs.h"

namespace nix_utils {
std::unique_ptr<InternalRealisation>
query_raw_realisation(const nix_utils::StoreWrapper &wrapper,
                      rust::Str output_id);
} // namespace nix_utils
