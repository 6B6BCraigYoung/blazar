#!/usr/bin/env bash
set -euo pipefail

NAME="${BLAZAR_SIGN_IDENTITY:-Blazar Local Signing}"
KEYCHAIN="${BLAZAR_SIGN_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
PASSFILE="${KEYCHAIN%.keychain-db}.pass"
OPENSSL=/usr/bin/openssl

dedicated() {
  [ -n "${BLAZAR_SIGN_KEYCHAIN:-}" ]
}

unlock() {
  if dedicated && [ -f "$PASSFILE" ]; then
    security unlock-keychain -p "$(cat "$PASSFILE")" "$KEYCHAIN"
  fi
}

has_identity() {
  [ -f "$KEYCHAIN" ] && security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1
}

ensure_keychain() {
  dedicated || return 0
  [ -f "$KEYCHAIN" ] && return 0
  mkdir -p "$(dirname "$KEYCHAIN")"
  (umask 077 && "$OPENSSL" rand -hex 24 >"$PASSFILE")
  security create-keychain -p "$(cat "$PASSFILE")" "$KEYCHAIN"
  security set-keychain-settings "$KEYCHAIN"
}

create() {
  ensure_keychain
  unlock
  if has_identity; then
    echo "已存在：$NAME"
    return
  fi
  local tmp
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  cat >"$tmp/req.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
EOF
  "$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/req.cnf" 2>/dev/null
  local pass
  pass=$("$OPENSSL" rand -hex 16)
  "$OPENSSL" pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
    -name "$NAME" -out "$tmp/identity.p12" -passout "pass:$pass"
  security import "$tmp/identity.p12" -k "$KEYCHAIN" -P "$pass" -T /usr/bin/codesign >/dev/null
  if dedicated; then
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
      -k "$(cat "$PASSFILE")" "$KEYCHAIN" >/dev/null
  fi
  echo "已创建：$NAME"
}

status() {
  if has_identity; then
    security find-certificate -c "$NAME" -Z "$KEYCHAIN" | awk '/SHA-1/ {print "证书指纹（SHA-1）：" $NF}'
    echo "签名身份：$NAME（$KEYCHAIN）"
  else
    echo "没有本地签名证书，打包时使用临时签名"
    return 1
  fi
}

remove() {
  if has_identity; then
    security delete-identity -c "$NAME" "$KEYCHAIN" >/dev/null
    echo "已删除：$NAME"
  fi
}

sign() {
  local target="$1"
  if has_identity; then
    unlock
    codesign --force --deep --keychain "$KEYCHAIN" --sign "$NAME" "$target"
    echo "已用 $NAME 签名：$target"
  else
    echo "没有本地签名证书，保留临时签名：$target"
  fi
}

case "${1:-status}" in
  create) create ;;
  status) status ;;
  remove) remove ;;
  sign) sign "${2:?用法：local-signing.sh sign <路径>}" ;;
  *) echo "用法：local-signing.sh create|status|remove|sign <路径>" >&2; exit 2 ;;
esac
