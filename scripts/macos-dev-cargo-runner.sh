#!/bin/sh
# Cargo 会把新编译的二进制路径及其参数传给此包装器；Node 脚本负责稳定重签名。
exec node "$(dirname "$0")/macos-dev-runner.mjs" "$@"
