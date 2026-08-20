#!/usr/bin/env bash
# End-to-end round-trip of encrypted PDFs through
# `shellac::ops::apply_ops`, driven via `shellac-cli`.
#
# lopdf#522 (shipped in `lopdf = "0.44.0"` on crates.io, the version this
# crate depends on) preserves /Encrypt across incremental saves and
# re-encrypts new objects under the loaded key. The engine's refusal is
# therefore split in two: `is_encrypted()` refuses with `EncryptedRefused`
# (a password is required — no encryption_state was retained, so there is
# no key to write under), and `was_encrypted() && !ANNOTABLE` refuses with
# `AnnotationsRestricted` (auto-decrypt succeeded but `/P` forbids
# annotation edits). The plain `shellac-cli add` / `shellac-cli remove`
# invocations below exercise the third case — empty-password, ANNOTABLE,
# encrypted — which must round-trip successfully.
#
# For each of four encryption variants (RC4-40, RC4-128, AES-128,
# AES-256):
#
#   1. Generate a small base PDF and qpdf-encrypt it (empty user pw
#      so lopdf's auto-decrypt on load succeeds).
#   2. Run `shellac-cli add` to append highlight + underline via
#      `shellac::ops::apply_ops`.
#   3. Run `qpdf --check` on the file — authoritative "is the
#      encryption + structure still sane after the increment?" test.
#   4. Run `shellac-cli check-ap` on the same post-add file — asserts
#      the Highlight carries a self-generated /AP /N Form XObject and
#      the Underline does NOT (the AP is Highlight-only). Runs before
#      `remove` so we exercise the AP round-trip through the encrypted
#      incremental writer.
#   5. Run `shellac-cli remove` — proves /NM lookup still resolves
#      after the encrypted increment.
#   6. Run `qpdf --check` again on the post-remove file: both halves of
#      the round-trip are checked, not just the add half.
#   7. Sample `tail -c 4096` and count `xref` (classic xref table) vs
#      `/Type /XRef` (XRef stream) markers so we can document what
#      lopdf emits for each variant.
#
# The script exits non-zero if any variant fails; a summary markdown
# table is printed to stdout.
#
# Style constraint (project convention): no `$(...)` command substitution
# and no backticks — every value is either a literal, set via a
# side-effecting function that assigns a global, or captured to a temp
# file and read back with `read`. `cd` is avoided in favor of absolute
# paths. Command substitution triggers an interactive confirmation in the
# agent tooling used on this repository, which breaks unattended runs.

set -euo pipefail

# -----------------------------------------------------------------------
# Locate the repository root without command substitution.
#
# Counting a fixed number of directory levels up from ${BASH_SOURCE[0]}
# does not work, because how many levels there are depends on how the
# script was invoked: `bash verify-encrypted-roundtrip.sh` from inside this
# directory yields `.`, which absolutizes to `$PWD/.` and adds a component
# that is not a directory level. Search for a marker instead — walk
# upwards until we find a directory that is the workspace root, and let the
# loop consume `.` and `..` components on the way.
#
# `rust-toolchain.toml` + `Cargo.toml` together are the marker: the crate
# directories under this root have a Cargo.toml but no toolchain file, so
# the pair cannot match anything but the workspace root.
# -----------------------------------------------------------------------
SCRIPT_PATH="${BASH_SOURCE[0]}"
SCRIPT_DIR="${SCRIPT_PATH%/*}"
# `${var%/*}` returns its input unchanged when there is no `/` to strip.
if [ "${SCRIPT_DIR}" = "${SCRIPT_PATH}" ]; then
    SCRIPT_DIR="."
fi
case "${SCRIPT_DIR}" in
    /*) ;;
    *) SCRIPT_DIR="${PWD}/${SCRIPT_DIR}" ;;
esac

REPO_ROOT=""
probe_dir="${SCRIPT_DIR}"
while [ -n "${probe_dir}" ]; do
    if [ -f "${probe_dir}/rust-toolchain.toml" ] && [ -f "${probe_dir}/Cargo.toml" ]; then
        REPO_ROOT="${probe_dir}"
        break
    fi
    probe_parent="${probe_dir%/*}"
    if [ "${probe_parent}" = "${probe_dir}" ]; then
        break
    fi
    probe_dir="${probe_parent}"
done
if [ -z "${REPO_ROOT}" ]; then
    echo "cannot locate the workspace root above ${SCRIPT_DIR}" >&2
    exit 2
fi
CARGO_MANIFEST="${REPO_ROOT}/Cargo.toml"

# -----------------------------------------------------------------------
# Temp working area. We build a deterministic path using $$ (PID) rather
# than `$(mktemp -d)` because command substitution is disallowed. If the
# path already exists it is emptied first; on exit it is trap-cleaned
# unless SHELLAC_KEEP_TMP is set.
# -----------------------------------------------------------------------
WORK_DIR="${TMPDIR:-/tmp}/shellac-encrypted-roundtrip.$$"
rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"

cleanup() {
    if [ -n "${SHELLAC_KEEP_TMP:-}" ]; then
        echo "SHELLAC_KEEP_TMP set; leaving ${WORK_DIR} in place" >&2
    else
        rm -rf "${WORK_DIR}"
    fi
}
trap cleanup EXIT

# -----------------------------------------------------------------------
# Base PDF — minimal 1-page classic-xref PDF, generated by writing raw
# bytes and running it through qpdf --qdf --object-streams=disable so
# the output is guaranteed to have a `xref` cross-reference table.
# lopdf preserves whichever xref format was in `prev`, so we start
# from classic-table for repeatability. Byte offsets in the xref must
# match the object placements exactly (verified with `wc -c`):
#   header %PDF-1.4\n%<binary>\n     → 0..14 (15 bytes)
#   obj 1  Catalog                    → 15..63  (49 bytes)
#   obj 2  Pages                      → 64..120 (57 bytes)
#   obj 3  Page                       → 121..208 (88 bytes)
#   xref                              → 209
# -----------------------------------------------------------------------
RAW_PDF="${WORK_DIR}/raw.pdf"
BASE_PDF="${WORK_DIR}/base.pdf"

printf '%%PDF-1.4\n%%\xE2\xE3\xCF\xD3\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>\nendobj\nxref\n0 4\n0000000000 65535 f \n0000000015 00000 n \n0000000064 00000 n \n0000000121 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n209\n%%%%EOF\n' > "${RAW_PDF}"

# Normalize with qpdf to guarantee a clean classic xref table. qpdf may
# exit 3 for stylistic warnings even on well-formed files; only exit 2
# (hard error) is fatal for our purposes.
set +e
qpdf --qdf --object-streams=disable --normalize-content=y "${RAW_PDF}" "${BASE_PDF}" >/dev/null 2>&1
qpdf_normalize_exit=$?
set -e
if [ "${qpdf_normalize_exit}" -ne 0 ] && [ "${qpdf_normalize_exit}" -ne 3 ]; then
    echo "qpdf normalize failed with exit ${qpdf_normalize_exit}" >&2
    exit 2
fi

# Sanity: base must be parseable.
set +e
qpdf --check "${BASE_PDF}" >/dev/null 2>&1
base_check_exit=$?
set -e
if [ "${base_check_exit}" -ne 0 ] && [ "${base_check_exit}" -ne 3 ]; then
    echo "base PDF failed qpdf --check with exit ${base_check_exit}" >&2
    exit 2
fi

# -----------------------------------------------------------------------
# Build shellac-cli once (release, --locked so CI parity holds).
# -----------------------------------------------------------------------
cargo build --manifest-path "${CARGO_MANIFEST}" --bin shellac-cli --release --locked >&2
CLI_BIN="${REPO_ROOT}/target/release/shellac-cli"
if [ ! -x "${CLI_BIN}" ]; then
    echo "shellac-cli binary not found at ${CLI_BIN}" >&2
    exit 2
fi

# -----------------------------------------------------------------------
# encrypt <variant> <extra qpdf-encrypt args...>
#
# Writes ${WORK_DIR}/enc-<variant>.pdf. Uses empty user password (so
# lopdf's auto-decrypt on load succeeds — a non-empty user password
# would leave every object encrypted after load, and every op would be
# refused up front by the `is_encrypted()` executor guard). Empty user
# + non-empty owner matches this crate's own encryption unit tests
# (see `fixture_aes_v4_empty_pw` in crates/shellac/src/tests.rs).
#
# RC4 40/128 need `--allow-weak-crypto` as a top-level qpdf flag (it
# must appear BEFORE `--encrypt`; it's not accepted inside the encrypt
# block). The two RC4 variants therefore call qpdf directly rather than
# routing through the shared helper.
# -----------------------------------------------------------------------
encrypt_aes() {
    local variant="$1"
    shift
    local dst="${WORK_DIR}/enc-${variant}.pdf"
    qpdf --encrypt "" owner "$@" -- "${BASE_PDF}" "${dst}"
}

encrypt_rc4() {
    local variant="$1"
    shift
    local dst="${WORK_DIR}/enc-${variant}.pdf"
    qpdf --allow-weak-crypto --encrypt "" owner "$@" -- "${BASE_PDF}" "${dst}"
}

# -----------------------------------------------------------------------
# Per-variant runner. Writes structured summary lines to $RESULTS.
# -----------------------------------------------------------------------
RESULTS="${WORK_DIR}/results.tsv"
: > "${RESULTS}"

# read_applied <log-file> <out-var>
#
# Pulls the last `"applied": N` value out of the shellac-cli stderr
# JSON and stores it in the named variable. Uses `grep | tail -n 1`
# piped into `read`, with the captured line spilled through a
# temp file (`${log}.applied`) — no command substitution or here-string.
read_applied() {
    local log="$1"
    local out_var="$2"
    local applied="0"
    local line=""
    # `grep -oE ... | tail -n 1` produces at most one line; we consume
    # it via `read`. `|| true` avoids the pipefail short-circuit when
    # grep matches nothing (empty log).
    { grep -oE '"applied":[[:space:]]*[0-9]+' "${log}" 2>/dev/null || true; } \
        | tail -n 1 \
        | { IFS= read -r line || true; printf '%s' "${line}"; } \
        > "${log}.applied"
    if [ -s "${log}.applied" ]; then
        local raw=""
        IFS= read -r raw < "${log}.applied"
        applied="${raw#*:}"
        applied="${applied// /}"
    fi
    printf -v "${out_var}" '%s' "${applied}"
}

# run_variant <name> <encrypted-pdf-path>
run_variant() {
    local name="$1"
    local pdf="$2"
    local add_log="${WORK_DIR}/${name}.add.log"
    local remove_log="${WORK_DIR}/${name}.remove.log"
    local qpdf_add_log="${WORK_DIR}/${name}.qpdf-check-add.log"
    local qpdf_rm_log="${WORK_DIR}/${name}.qpdf-check-remove.log"
    local ap_log="${WORK_DIR}/${name}.check-ap.log"
    local xref_log="${WORK_DIR}/${name}.xref.log"

    echo "== ${name} ==" >&2

    if [ ! -f "${pdf}" ]; then
        # Missing encrypted-input file (qpdf pre-step failed). Keep the
        # column types stable so downstream readers can parse both the
        # happy and unhappy paths the same way: status columns
        # (add/check-add/ap/remove/check-remove) get "enc-fail", integer
        # columns (add-applied/remove-applied/xref hits) get "0".
        printf '%s\tenc-fail\t0\tenc-fail\tenc-fail\tenc-fail\t0\tenc-fail\t0\t0\n' \
            "${name}" >> "${RESULTS}"
        return 1
    fi

    # 1. shellac-cli add (highlight + underline).
    local add_status="fail"
    local add_applied="0"
    local add_exit="1"
    set +e
    "${CLI_BIN}" add "${pdf}" >"${add_log}" 2>&1
    add_exit=$?
    set -e
    read_applied "${add_log}" add_applied
    if [ "${add_exit}" -eq 0 ]; then
        add_status="ok"
    fi

    # 2. qpdf --check on the post-add file. qpdf exits 0 on clean,
    # 3 on warnings (file still valid — treated as "ok-warn"), 2 on
    # structural failure. Empty user password → `--password=` (empty).
    local check_add_status="fail"
    local check_add_exit="1"
    set +e
    qpdf --check --password= "${pdf}" >"${qpdf_add_log}" 2>&1
    check_add_exit=$?
    set -e
    if [ "${check_add_exit}" -eq 0 ]; then
        check_add_status="ok"
    elif [ "${check_add_exit}" -eq 3 ]; then
        check_add_status="ok-warn"
    fi

    # 3. shellac-cli check-ap — asserts the Highlight carries a
    # self-generated /AP /N Form XObject and the Underline does not.
    # Runs on the encrypted post-add file so we also exercise the AP
    # stream round-trip through the incremental writer's re-encryption
    # path.
    local ap_status="fail"
    local ap_exit="1"
    set +e
    "${CLI_BIN}" check-ap "${pdf}" >"${ap_log}" 2>&1
    ap_exit=$?
    set -e
    if [ "${ap_exit}" -eq 0 ]; then
        ap_status="ok"
    fi

    # 4. shellac-cli remove — confirms /NM lookup + second-round
    # increment work after the first encrypted increment.
    local remove_status="fail"
    local remove_applied="0"
    local remove_exit="1"
    set +e
    "${CLI_BIN}" remove "${pdf}" >"${remove_log}" 2>&1
    remove_exit=$?
    set -e
    read_applied "${remove_log}" remove_applied
    if [ "${remove_exit}" -eq 0 ]; then
        remove_status="ok"
    fi

    # 5. qpdf --check on the post-remove file: the second half of the
    # round-trip is checked too, not just the add half. Same exit-code
    # semantics as above.
    local check_rm_status="fail"
    local check_rm_exit="1"
    set +e
    qpdf --check --password= "${pdf}" >"${qpdf_rm_log}" 2>&1
    check_rm_exit=$?
    set -e
    if [ "${check_rm_exit}" -eq 0 ]; then
        check_rm_status="ok"
    elif [ "${check_rm_exit}" -eq 3 ]; then
        check_rm_status="ok-warn"
    fi

    # 6. xref-format observation. `tail -c 4096` + grep is enough since
    # incremental writes append a fresh xref section at EOF. Counts are
    # captured to files (avoiding $(...)) and read back via `read`.
    local xref_table_hits="0"
    local xref_stream_hits="0"
    tail -c 4096 "${pdf}" > "${xref_log}"
    # Classic xref table starts with `xref` on its own line; XRef stream
    # dicts carry /Type /XRef.
    if ! grep -c '^xref$' "${xref_log}" > "${xref_log}.tcount" 2>/dev/null; then
        echo 0 > "${xref_log}.tcount"
    fi
    if ! grep -cE '/Type[[:space:]]*/XRef' "${xref_log}" > "${xref_log}.scount" 2>/dev/null; then
        echo 0 > "${xref_log}.scount"
    fi
    IFS= read -r xref_table_hits < "${xref_log}.tcount"
    IFS= read -r xref_stream_hits < "${xref_log}.scount"

    # Record TSV row.
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "${name}" \
        "${add_status}" \
        "${add_applied}" \
        "${check_add_status}" \
        "${ap_status}" \
        "${remove_status}" \
        "${remove_applied}" \
        "${check_rm_status}" \
        "${xref_table_hits}" \
        "${xref_stream_hits}" \
        >> "${RESULTS}"

    # Aggregate failure detection: add/remove must be ok; both qpdf
    # --check calls must be ok OR ok-warn; check-ap must be ok;
    # AND both add and remove must have applied exactly 2 ops
    # (highlight + underline). shellac-cli reports SUCCESS for
    # applied > 0, so without the applied==2 gate a regression that
    # silently drops one of the two annotations would slip through
    # here as "all four variants passed".
    if [ "${add_status}" != "ok" ] || [ "${remove_status}" != "ok" ]; then
        return 1
    fi
    if [ "${check_add_status}" != "ok" ] && [ "${check_add_status}" != "ok-warn" ]; then
        return 1
    fi
    if [ "${check_rm_status}" != "ok" ] && [ "${check_rm_status}" != "ok-warn" ]; then
        return 1
    fi
    if [ "${ap_status}" != "ok" ]; then
        echo "${name}: check-ap failed" >&2
        return 1
    fi
    if [ "${add_applied}" != "2" ] || [ "${remove_applied}" != "2" ]; then
        echo "${name}: expected applied=2, got add=${add_applied} remove=${remove_applied}" >&2
        return 1
    fi
    return 0
}

# -----------------------------------------------------------------------
# Encrypt & run all four variants. Track pass/fail without aborting so
# the final table shows the complete picture.
# -----------------------------------------------------------------------
overall_fail=0

encrypt_rc4  rc4-40  40  --print=y --modify=y
if ! run_variant "rc4-40" "${WORK_DIR}/enc-rc4-40.pdf"; then overall_fail=1; fi

encrypt_rc4  rc4-128 128 --use-aes=n
if ! run_variant "rc4-128" "${WORK_DIR}/enc-rc4-128.pdf"; then overall_fail=1; fi

encrypt_aes  aes-128 128 --use-aes=y
if ! run_variant "aes-128" "${WORK_DIR}/enc-aes-128.pdf"; then overall_fail=1; fi

encrypt_aes  aes-256 256
if ! run_variant "aes-256" "${WORK_DIR}/enc-aes-256.pdf"; then overall_fail=1; fi

# -----------------------------------------------------------------------
# Emit summary markdown table to stdout.
# -----------------------------------------------------------------------
echo ""
echo "## verify-encrypted-roundtrip.sh results"
echo ""
echo "| variant | add | applied | qpdf --check (add) | check-ap | remove | applied | qpdf --check (remove) | xref-table lines | XRef-stream refs |"
echo "|---------|-----|---------|--------------------|----------|--------|---------|-----------------------|------------------|------------------|"
while IFS=$'\t' read -r variant add applied check_add ap remove rapplied check_rm t s; do
    printf '| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' \
        "${variant}" "${add}" "${applied}" "${check_add}" "${ap}" "${remove}" "${rapplied}" "${check_rm}" "${t}" "${s}"
done < "${RESULTS}"

echo ""
if [ "${overall_fail}" -ne 0 ]; then
    echo "verify-encrypted-roundtrip.sh: at least one variant failed." >&2
    exit 1
fi
echo "verify-encrypted-roundtrip.sh: all four variants passed."
