#!/usr/bin/env bash
# generate_encrypted_fixtures.sh — derive S9..S12 from S1 by re-encrypting
# with qpdf.
#
# Idempotent: any target file that already exists is left in place. This
# lets a matrix rerun call the script freely without clobbering
# hand-curated variants or forcing qpdf work again.
#
# Four fixtures, all derived from <samples_dir>/S1.pdf:
#   S9-rc4-empty-user.pdf
#       RC4 R3 (128-bit), empty user password, owner password set.
#       Expected behavior of the save executor under test: success —
#       lopdf#521/#522 preserves /Encrypt across incremental saves and
#       re-encrypts new objects under the loaded key; the harness runs
#       the full add/remove/loop round-trip on this file.
#   S10-aes256-empty-user.pdf
#       AESV3 R6 (256-bit), empty user password, owner password set.
#       Same expected behavior as S9.
#   S11-password-required.pdf
#       AESV3 R6 with a non-empty user password. The save executor
#       under test cannot auto-decrypt, so `apply_ops` refuses with
#       Status::EncryptedRefused and the file stays byte-for-byte
#       unchanged. The harness treats "refused + bytes unchanged +
#       status matches expected" as pass.
#   S12-annotations-restricted.pdf
#       AESV3 R6, empty user password, but /P clears the ANNOTABLE bit
#       (qpdf --annotate=n --modify=none). The save executor under test
#       auto-decrypts, then refuses annotation ops with
#       Status::AnnotationsRestricted; file stays byte-for-byte
#       unchanged. Same refused-unchanged treatment by the harness.
#
# Bash-style constraints (project convention: no command substitution and
# no backticks — see generate_s3.sh for the same rule): every value is
# either a literal, a positional argument, or captured to a temp file and
# read back with `read`.
#
# Usage: bash corpus/generators/generate_encrypted_fixtures.sh <samples_dir>

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: bash corpus/generators/generate_encrypted_fixtures.sh <samples_dir>" >&2
    exit 1
fi
samples_dir="$1"

if [ ! -d "${samples_dir}" ]; then
    echo "generate_encrypted_fixtures: samples dir does not exist: ${samples_dir}" >&2
    exit 1
fi

src="${samples_dir}/S1.pdf"
if [ ! -f "${src}" ]; then
    echo "generate_encrypted_fixtures: source PDF not found: ${src}" >&2
    exit 1
fi

if ! command -v qpdf >/dev/null 2>&1; then
    echo "generate_encrypted_fixtures: qpdf not on PATH (brew install qpdf)" >&2
    exit 1
fi

# generate_fixture <target-name> <mode>
#   mode = one of:
#     rc4-empty      → qpdf --allow-weak-crypto --encrypt "" owner 128 --use-aes=n
#     aes256-empty   → qpdf --encrypt "" owner 256
#     aes256-userpw  → qpdf --encrypt user owner 256   (user pw is set)
#     aes256-noannot → qpdf --encrypt "" owner 256 --annotate=n --modify=none
# owner password is a literal (`shellac-verify-owner`) rather than empty so
# the /P bits actually take effect (qpdf ignores restrictions when owner
# is also empty). The save executor under test never uses the owner
# password, only the empty user pw.
generate_fixture() {
    local name="$1"
    local mode="$2"
    local dst="${samples_dir}/${name}"

    if [ -f "${dst}" ]; then
        echo "generate_encrypted_fixtures: ${name} already exists, skipping"
        return 0
    fi

    case "${mode}" in
        rc4-empty)
            echo "generate_encrypted_fixtures: creating ${name} (RC4-128, empty user pw)"
            qpdf --allow-weak-crypto --encrypt "" shellac-verify-owner 128 --use-aes=n -- "${src}" "${dst}"
            ;;
        aes256-empty)
            echo "generate_encrypted_fixtures: creating ${name} (AES-256, empty user pw)"
            qpdf --encrypt "" shellac-verify-owner 256 -- "${src}" "${dst}"
            ;;
        aes256-userpw)
            echo "generate_encrypted_fixtures: creating ${name} (AES-256, user pw required)"
            qpdf --encrypt shellac-verify-user shellac-verify-owner 256 -- "${src}" "${dst}"
            ;;
        aes256-noannot)
            echo "generate_encrypted_fixtures: creating ${name} (AES-256, empty user pw, /P annotate=n modify=none)"
            qpdf --encrypt "" shellac-verify-owner 256 --annotate=n --modify=none -- "${src}" "${dst}"
            ;;
        *)
            echo "generate_encrypted_fixtures: unknown mode ${mode}" >&2
            return 1
            ;;
    esac
}

generate_fixture "S9-rc4-empty-user.pdf"          rc4-empty
generate_fixture "S10-aes256-empty-user.pdf"      aes256-empty
generate_fixture "S11-password-required.pdf"      aes256-userpw
generate_fixture "S12-annotations-restricted.pdf" aes256-noannot

echo "generate_encrypted_fixtures: done"
