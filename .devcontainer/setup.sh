## update and install some things we should probably have
apt-get update
apt-get upgrade -y
apt-get install -y \
  apt-utils \
  curl \
  git \
  git-lfs \
  gnupg2 \
  jq \
  build-essential \
  openssl \
  libssl-dev \
  pkg-config \
  cmake \
  wget \
  file \
  ca-certificates \
  zstd \
  clang \
  lld \
  protobuf-compiler \
  seccomp \
  libseccomp-dev 
  
## Install rustup and common components
curl https://sh.rustup.rs -sSf | sh -s -- -y
source /root/.cargo/env

## Init git-lfs
git lfs install
