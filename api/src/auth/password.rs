//! Password hashing: argon2id for new hashes, Werkzeug-compatible
//! verification for existing rows.
//!
//! Legacy stored format (rows written by Flask, or by this server before
//! the argon2 migration) is Werkzeug's `method$salt$hexdigest` where
//! `method` is `scrypt:<n>:<r>:<p>` (dklen 64) or
//! `pbkdf2:<hash>:<iterations>` with `hash` = sha256 (dklen 32) — a
//! faithful port of `werkzeug/security.py` (`check_password_hash`).
//! Existing rows must keep verifying forever.
//!
//! New hashes are argon2id in PHC string format (`$argon2id$...`),
//! RustCrypto `argon2` default params. The original port emitted
//! Werkzeug's scrypt format so a rollback to Flask kept working; the
//! cutover is complete and that rollback constraint is retired, so new
//! hashes upgrade to argon2id and legacy hashes are transparently
//! rehashed on successful login (see [`needs_rehash`] and the
//! credentialed-login branch in `routes/auth.rs`).
//!
//! Performance note: Werkzeug's default `scrypt:32768:8:1` costs ~32 MB
//! and tens of milliseconds per call, and argon2id's defaults are in the
//! same class. These functions are intentionally synchronous and
//! CPU-bound — HTTP callers must run them on
//! `tokio::task::spawn_blocking`, never on the async executor.

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use scrypt::Params;
use subtle::ConstantTimeEq;

/// PHC prefix written by [`generate_password_hash`]; anything else stored
/// in the DB is a legacy Werkzeug hash (or garbage) and should be rehashed
/// on the next successful login.
const ARGON2ID_PREFIX: &str = "$argon2id$";

/// Werkzeug's `SALT_CHARS`.
const SALT_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Werkzeug's `DEFAULT_PBKDF2_ITERATIONS` (3.1+): used when a stored
/// method string omits the iteration count (`pbkdf2` / `pbkdf2:sha256`).
pub const DEFAULT_PBKDF2_ITERATIONS: u32 = 1_000_000;

/// Werkzeug's default salt length for its `generate_password_hash` (used
/// with [`generate_password_hash_with`] when seeding legacy-format rows).
pub const DEFAULT_SALT_LENGTH: usize = 16;

/// Both digests Werkzeug emits are fixed-length: `hashlib.scrypt` defaults
/// to dklen 64, `hashlib.pbkdf2_hmac("sha256", ...)` to the digest size 32.
const SCRYPT_DKLEN: usize = 64;
const PBKDF2_SHA256_DKLEN: usize = 32;

/// A parsed Werkzeug method string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashMethod {
    /// `scrypt:<n>:<r>:<p>`; bare `scrypt` means `scrypt:32768:8:1`.
    Scrypt { n: u32, r: u32, p: u32 },
    /// `pbkdf2:sha256:<iterations>`; bare `pbkdf2` / `pbkdf2:sha256`
    /// default to [`DEFAULT_PBKDF2_ITERATIONS`].
    Pbkdf2Sha256 { iterations: u32 },
}

impl HashMethod {
    /// Werkzeug's default method for new hashes (`generate_password_hash`).
    pub const DEFAULT: HashMethod = HashMethod::Scrypt {
        n: 32768,
        r: 8,
        p: 1,
    };

    /// The normalized method prefix Werkzeug writes into the stored hash
    /// (`_hash_internal`'s `actual_method`).
    fn actual_method(&self) -> String {
        match self {
            HashMethod::Scrypt { n, r, p } => format!("scrypt:{n}:{r}:{p}"),
            HashMethod::Pbkdf2Sha256 { iterations } => format!("pbkdf2:sha256:{iterations}"),
        }
    }
}

/// Failure modes of parsing/deriving. Mirrors where Werkzeug raises
/// `ValueError` (which Flask surfaces as a 500, never a "wrong password").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordHashError {
    /// Unknown method name (Werkzeug: `Invalid hash method '<m>'.`).
    #[error("Invalid hash method '{0}'.")]
    InvalidMethod(String),
    /// `scrypt:` with anything other than three integer args.
    #[error("'scrypt' takes 3 arguments.")]
    ScryptArgs,
    /// `pbkdf2:` with more than two args.
    #[error("'pbkdf2' takes 2 arguments.")]
    Pbkdf2Args,
    /// Non-integer iteration count (Python: uncaught `int()` ValueError).
    #[error("invalid pbkdf2 iteration count")]
    InvalidIterations,
    /// A digest other than sha256. Werkzeug delegates to hashlib and would
    /// accept e.g. sha512, but Nightlio has only ever written sha256;
    /// anything else is unsupported here rather than silently wrong.
    #[error("unsupported pbkdf2 digest '{0}'")]
    UnsupportedDigest(String),
    /// scrypt parameters hashlib/the scrypt crate reject (n not a power of
    /// two, zero r/p, ...).
    #[error("invalid scrypt parameters")]
    InvalidScryptParams,
    /// A stored value with the `$argon2id$` prefix that the argon2 crate
    /// cannot parse/process — a data problem (like Werkzeug's
    /// `ValueError`s above), never reported as "wrong password".
    #[error("invalid argon2 hash")]
    InvalidArgon2Hash,
}

/// Parse a Werkzeug method string (`scrypt:32768:8:1`,
/// `pbkdf2:sha256:600000`, bare `scrypt` / `pbkdf2` / `pbkdf2:sha256`).
/// Port of the dispatch in `werkzeug.security._hash_internal`.
pub fn parse_method(method: &str) -> Result<HashMethod, PasswordHashError> {
    let mut parts = method.split(':');
    let name = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    match name {
        "scrypt" => {
            if args.is_empty() {
                return Ok(HashMethod::DEFAULT);
            }
            if args.len() != 3 {
                return Err(PasswordHashError::ScryptArgs);
            }
            let parse = |s: &str| s.parse::<u32>().map_err(|_| PasswordHashError::ScryptArgs);
            Ok(HashMethod::Scrypt {
                n: parse(args[0])?,
                r: parse(args[1])?,
                p: parse(args[2])?,
            })
        }
        "pbkdf2" => {
            let (hash_name, iterations) = match args.len() {
                0 => ("sha256", DEFAULT_PBKDF2_ITERATIONS),
                1 => (args[0], DEFAULT_PBKDF2_ITERATIONS),
                2 => (
                    args[0],
                    args[1]
                        .parse()
                        .map_err(|_| PasswordHashError::InvalidIterations)?,
                ),
                _ => return Err(PasswordHashError::Pbkdf2Args),
            };
            if hash_name != "sha256" {
                return Err(PasswordHashError::UnsupportedDigest(hash_name.to_string()));
            }
            Ok(HashMethod::Pbkdf2Sha256 { iterations })
        }
        other => Err(PasswordHashError::InvalidMethod(other.to_string())),
    }
}

/// Derive the lowercase hex digest for `password` under `method` and
/// `salt` — Werkzeug's `_hash_internal`, minus the salt generation.
fn derive_hex(method: HashMethod, salt: &str, password: &str) -> Result<String, PasswordHashError> {
    match method {
        HashMethod::Scrypt { n, r, p } => {
            // hashlib.scrypt requires n to be a power of two > 1.
            if n < 2 || !n.is_power_of_two() {
                return Err(PasswordHashError::InvalidScryptParams);
            }
            let log_n = n.trailing_zeros() as u8;
            let params =
                Params::new(log_n, r, p).map_err(|_| PasswordHashError::InvalidScryptParams)?;
            let mut output = [0u8; SCRYPT_DKLEN];
            scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut output)
                .map_err(|_| PasswordHashError::InvalidScryptParams)?;
            Ok(hex_lower(&output))
        }
        HashMethod::Pbkdf2Sha256 { iterations } => {
            let mut output = [0u8; PBKDF2_SHA256_DKLEN];
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                password.as_bytes(),
                salt.as_bytes(),
                iterations,
                &mut output,
            );
            Ok(hex_lower(&output))
        }
    }
}

/// Check `password` against a stored hash — argon2id PHC strings (new
/// format) or legacy Werkzeug hashes.
///
/// The Werkzeug branch is a port of `werkzeug.security.check_password_hash`:
/// - a value without two `$` separators is simply "no match" (`Ok(false)`);
/// - an unparseable/unsupported method is an error (Werkzeug raises
///   `ValueError` there — a data problem, not a wrong password);
/// - the digest comparison is constant-time (`hmac.compare_digest`;
///   argon2's `verify_password` is likewise constant-time via `subtle`).
pub fn check_password_hash(pwhash: &str, password: &str) -> Result<bool, PasswordHashError> {
    if pwhash.starts_with(ARGON2ID_PREFIX) {
        let parsed = PasswordHash::new(pwhash).map_err(|_| PasswordHashError::InvalidArgon2Hash)?;
        return match Argon2::default().verify_password(password.as_bytes(), &parsed) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(_) => Err(PasswordHashError::InvalidArgon2Hash),
        };
    }
    let mut parts = pwhash.splitn(3, '$');
    let (Some(method), Some(salt), Some(hashval)) = (parts.next(), parts.next(), parts.next())
    else {
        return Ok(false);
    };
    let expected = derive_hex(parse_method(method)?, salt, password)?;
    // subtle's slice ct_eq short-circuits only on length, exactly like
    // hmac.compare_digest.
    Ok(expected.as_bytes().ct_eq(hashval.as_bytes()).into())
}

/// Hash a new password as argon2id (PHC string, RustCrypto `argon2`
/// default params, fresh random salt).
///
/// New hashes are argon2 now because the Flask rollback constraint is
/// retired: while a rollback to the Python server had to stay possible,
/// this emitted Werkzeug's `scrypt:32768:8:1$...` format (Flask cannot
/// verify PHC strings). With the cutover complete, only this server reads
/// these rows, so new passwords — and legacy rows via rehash-on-login
/// ([`needs_rehash`]) — upgrade to argon2id.
pub fn generate_password_hash(password: &str) -> String {
    // Salt from the crate-wide `rand` (OS-seeded CSPRNG) rather than
    // `SaltString::generate(&mut OsRng)`: password-hash's `OsRng` re-export
    // sits behind a feature flag we would only get via feature
    // unification. 16 bytes = password_hash's RECOMMENDED length, the same
    // amount `SaltString::generate` draws.
    let salt_bytes: [u8; 16] = {
        use rand::RngExt;
        rand::rng().random()
    };
    let salt = SaltString::encode_b64(&salt_bytes).expect("16 bytes always fit a SaltString");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 default parameters are always valid")
        .to_string()
}

/// True when a stored hash should be transparently upgraded to argon2id
/// after the plaintext has been verified against it — i.e. anything that
/// is not already an argon2id PHC string (legacy Werkzeug scrypt/pbkdf2).
pub fn needs_rehash(stored: &str) -> bool {
    !stored.starts_with(ARGON2ID_PREFIX)
}

/// Werkzeug-format `generate_password_hash` with an explicit method and
/// salt length — kept for tests (fixture regeneration, legacy-row
/// seeding); production code writes argon2id via
/// [`generate_password_hash`].
pub fn generate_password_hash_with(
    password: &str,
    method: HashMethod,
    salt_length: usize,
) -> Result<String, PasswordHashError> {
    let salt = gen_salt(salt_length);
    let digest = derive_hex(method, &salt, password)?;
    Ok(format!("{}${salt}${digest}", method.actual_method()))
}

/// Werkzeug's `gen_salt`: `length` characters drawn from [`SALT_CHARS`].
fn gen_salt(length: usize) -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..length)
        .map(|_| SALT_CHARS[rng.random_range(0..SALT_CHARS.len())] as char)
        .collect()
}

/// Lowercase hex, matching Python's `bytes.hex()`.
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(out, "{b:02x}").expect("writing to a String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Reference hashes pinned from `werkzeug.security.generate_password_hash`
    /// in the Flask venv (api/venv) — the exact library that wrote every
    /// existing row — generated while the venv still existed. Covers
    /// full-strength `scrypt:32768:8:1`, `pbkdf2:sha256` at 600000 and
    /// 1000000 iterations, and cheap variants; each record carries the
    /// plaintext, wrong-password probes, and werkzeug's verdicts asserted
    /// at generation time.
    const WERKZEUG_FIXTURE: &str = include_str!("../../tests/fixtures/werkzeug_hashes.json");

    /// Rollback-safety records: hashes generated by *this module* that the
    /// venv's real `werkzeug.security.check_password_hash` accepted at
    /// fixture-generation time (`werkzeug_accepted: true`).
    const ACCEPTANCE_FIXTURE: &str = include_str!("../../tests/fixtures/acceptance.json");

    // Cheap-parameter pinned hashes (also werkzeug-generated) for
    // behavioral tests, so the suite is not dominated by full-cost KDF
    // runs.
    const SCRYPT_CHEAP: &str = "scrypt:2048:8:1$P1i4zOFBqQhafL14$1c3a74a3b82c619fd0299c6f5386f95d689f95a2389da299852ca3a294a6c29735f75ef07f465c2a51996bd03a78fab4e20624f8550a88cbb39c317c42a4cb9c";
    const PBKDF2_CHEAP: &str = "pbkdf2:sha256:1000$9mGk3jaNAJBbWfu4$a956000d41c9a2a08ee917585fe66311acd661744951aac17d3aa9a1e89aca0f";
    const CHEAP_PASSWORD: &str = "hunter2";

    /// `_hash_internal("pbkdf2:sha256", "fixedsalt", "hunter2")` — a stored
    /// method string without an iteration count verifies against
    /// DEFAULT_PBKDF2_ITERATIONS (1_000_000), confirmed against werkzeug.
    const PBKDF2_NO_ITERS: &str =
        "pbkdf2:sha256$fixedsalt$2d2ef8879ead29df818c3fa9e8727810839d8acc9e75c7e55af32b2b4a777014";

    /// Every pinned werkzeug hash in the fixture set verifies with its
    /// plaintext and rejects the wrong-password probes — covering
    /// full-strength `scrypt:32768:8:1`, `pbkdf2:sha256:600000`,
    /// `pbkdf2:sha256:1000000`, and the cheap variants.
    #[test]
    fn verifies_werkzeug_fixture_hashes() {
        let fixture: serde_json::Value = serde_json::from_str(WERKZEUG_FIXTURE).unwrap();
        let records = fixture["hashes"].as_array().unwrap();
        let names: Vec<&str> = records
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "scrypt_full",
                "pbkdf2_600k",
                "pbkdf2_1m",
                "scrypt_cheap",
                "pbkdf2_cheap"
            ]
        );
        for record in records {
            let name = record["name"].as_str().unwrap();
            let hash = record["hash"].as_str().unwrap();
            let password = record["password"].as_str().unwrap();
            assert!(hash.starts_with(record["method"].as_str().unwrap()));
            assert_eq!(record["werkzeug_accepts_password"], serde_json::json!(true));
            assert_eq!(
                check_password_hash(hash, password),
                Ok(true),
                "fixture {name} did not verify"
            );
            for wrong in record["wrong_passwords"].as_array().unwrap() {
                assert_eq!(
                    check_password_hash(hash, wrong.as_str().unwrap()),
                    Ok(false),
                    "fixture {name} accepted a wrong password"
                );
            }
        }
    }

    #[test]
    fn pbkdf2_without_iteration_count_uses_default_iterations() {
        assert_eq!(check_password_hash(PBKDF2_NO_ITERS, "hunter2"), Ok(true));
    }

    #[rstest]
    #[case::scrypt(SCRYPT_CHEAP)]
    #[case::pbkdf2(PBKDF2_CHEAP)]
    fn rejects_wrong_password(#[case] pwhash: &str) {
        assert_eq!(check_password_hash(pwhash, "hunter3"), Ok(false));
        assert_eq!(check_password_hash(pwhash, ""), Ok(false));
    }

    #[test]
    fn malformed_stored_value_is_no_match_not_an_error() {
        // werkzeug: pwhash.split("$", 2) unpack failure -> False.
        assert_eq!(check_password_hash("", "pw"), Ok(false));
        assert_eq!(check_password_hash("no-dollar-signs", "pw"), Ok(false));
        assert_eq!(
            check_password_hash("scrypt:2048:8:1$onlysalt", "pw"),
            Ok(false)
        );
    }

    #[test]
    fn unknown_method_is_an_error_with_werkzeug_message() {
        // werkzeug raises ValueError("Invalid hash method 'md5'.") here.
        let err = check_password_hash("md5$salt$abcdef", "pw").unwrap_err();
        assert_eq!(err, PasswordHashError::InvalidMethod("md5".to_string()));
        assert_eq!(err.to_string(), "Invalid hash method 'md5'.");
    }

    #[rstest]
    #[case::two_args("scrypt:2048:8$s$d", PasswordHashError::ScryptArgs)]
    #[case::non_int("scrypt:a:b:c$s$d", PasswordHashError::ScryptArgs)]
    #[case::four_args_pbkdf2("pbkdf2:sha256:1000:extra$s$d", PasswordHashError::Pbkdf2Args)]
    #[case::bad_iters("pbkdf2:sha256:lots$s$d", PasswordHashError::InvalidIterations)]
    #[case::sha512("pbkdf2:sha512:1000$s$d", PasswordHashError::UnsupportedDigest("sha512".into()))]
    #[case::non_pow2_n("scrypt:1000:8:1$s$d", PasswordHashError::InvalidScryptParams)]
    fn bad_method_strings_error(#[case] pwhash: &str, #[case] expected: PasswordHashError) {
        assert_eq!(check_password_hash(pwhash, "pw").unwrap_err(), expected);
    }

    #[test]
    fn extra_dollar_signs_stay_in_the_digest_and_fail_cleanly() {
        // split("$", 2) keeps everything after the second "$" as hashval.
        let tampered = format!("{PBKDF2_CHEAP}$junk");
        assert_eq!(check_password_hash(&tampered, CHEAP_PASSWORD), Ok(false));
    }

    #[test]
    fn generated_hash_is_argon2id_and_round_trips() {
        let hash = generate_password_hash("s3cret pa55word");
        assert!(
            hash.starts_with("$argon2id$"),
            "new hashes must be argon2id PHC strings, got {hash}"
        );
        assert!(!needs_rehash(&hash));
        assert_eq!(check_password_hash(&hash, "s3cret pa55word"), Ok(true));
        assert_eq!(check_password_hash(&hash, "wrong"), Ok(false));
        assert_eq!(check_password_hash(&hash, ""), Ok(false));
    }

    #[test]
    fn generated_argon2_hashes_are_salted_uniquely() {
        let first = generate_password_hash("same password");
        let second = generate_password_hash("same password");
        assert_ne!(first, second, "fresh random salt per hash");
        assert_eq!(check_password_hash(&second, "same password"), Ok(true));
    }

    /// A pinned PHC string verifies (not just round-trip of today's code):
    /// generated once with `Argon2::default()` (v19, m=19456,t=2,p=1) for
    /// password `hunter2`.
    #[test]
    fn pinned_argon2id_hash_verifies() {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw$YKyQnOC5V1VGBUUiy3+cYg1ACUwGPnP6VMRgR0vdBWA";
        assert_eq!(check_password_hash(hash, "hunter2"), Ok(true));
        assert_eq!(check_password_hash(hash, "hunter3"), Ok(false));
    }

    #[test]
    fn malformed_argon2id_value_is_an_error_not_wrong_password() {
        // `!` is outside the PHC B64/ident charset → PasswordHash::new
        // fails → data problem, like Werkzeug's ValueError branch.
        assert_eq!(
            check_password_hash("$argon2id$!!!not-phc!!!", "pw"),
            Err(PasswordHashError::InvalidArgon2Hash)
        );
        // A parseable PHC string with a salt but no digest: the argon2
        // crate reports that as Error::Password, so it surfaces as "no
        // match" rather than a 500 — acceptable (nothing we wrote ever
        // looks like this).
        assert_eq!(
            check_password_hash("$argon2id$not-a-valid-phc-string", "pw"),
            Ok(false)
        );
    }

    #[rstest]
    #[case::scrypt_full("scrypt:32768:8:1$P1i4zOFBqQhafL14$00", true)]
    #[case::scrypt_cheap(SCRYPT_CHEAP, true)]
    #[case::pbkdf2_cheap(PBKDF2_CHEAP, true)]
    #[case::pbkdf2_no_iters(PBKDF2_NO_ITERS, true)]
    #[case::empty("", true)]
    #[case::garbage("no-dollar-signs", true)]
    #[case::argon2i_not_id("$argon2i$v=19$m=19456,t=2,p=1$c2FsdA$AA", true)]
    #[case::argon2id(
        "$argon2id$v=19$m=19456,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw$YKyQnOC5V1VGBUUiy3+cYg1ACUwGPnP6VMRgR0vdBWA",
        false
    )]
    fn needs_rehash_matrix(#[case] stored: &str, #[case] expected: bool) {
        assert_eq!(needs_rehash(stored), expected);
    }

    #[test]
    fn needs_rehash_true_for_fresh_werkzeug_false_for_fresh_argon2() {
        let legacy =
            generate_password_hash_with("pw", HashMethod::Pbkdf2Sha256 { iterations: 1000 }, 16)
                .unwrap();
        assert!(needs_rehash(&legacy));
        assert!(!needs_rehash(&generate_password_hash("pw")));
    }

    #[test]
    fn werkzeug_generator_still_emits_werkzeug_default_format() {
        // The legacy generator stays available (and correct) for seeding
        // legacy rows in tests.
        let hash = generate_password_hash_with(
            "s3cret pa55word",
            HashMethod::DEFAULT,
            DEFAULT_SALT_LENGTH,
        )
        .unwrap();
        let (method, rest) = hash.split_once('$').unwrap();
        let (salt, digest) = rest.split_once('$').unwrap();
        assert_eq!(method, "scrypt:32768:8:1");
        assert_eq!(salt.len(), 16);
        assert!(salt.bytes().all(|b| SALT_CHARS.contains(&b)));
        assert_eq!(digest.len(), SCRYPT_DKLEN * 2);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(check_password_hash(&hash, "s3cret pa55word"), Ok(true));
        assert_eq!(check_password_hash(&hash, "wrong"), Ok(false));
    }

    #[test]
    fn generated_pbkdf2_hash_self_verifies() {
        let hash = generate_password_hash_with(
            "another pw",
            HashMethod::Pbkdf2Sha256 { iterations: 1000 },
            16,
        )
        .unwrap();
        assert!(hash.starts_with("pbkdf2:sha256:1000$"));
        assert_eq!(check_password_hash(&hash, "another pw"), Ok(true));
    }

    /// Rollback safety, pinned: these hashes were generated by *this
    /// module* and verified by the venv's real
    /// `werkzeug.security.check_password_hash` at fixture-generation time
    /// (`werkzeug_accepted: true`, wrong password rejected). Here we
    /// re-assert that today's code still verifies its own historical
    /// output — hashes are salted, so byte-identical regeneration is not
    /// possible; self-verification plus the pinned werkzeug verdict is the
    /// invariant.
    #[test]
    fn werkzeug_accepted_rust_generated_hashes() {
        let fixture: serde_json::Value = serde_json::from_str(ACCEPTANCE_FIXTURE).unwrap();
        let records = fixture["werkzeug"].as_array().unwrap();
        let names: Vec<&str> = records
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["rust_scrypt_default", "rust_pbkdf2_1000"]);
        for record in records {
            assert_eq!(record["werkzeug_accepted"], serde_json::json!(true));
            assert_eq!(
                record["werkzeug_rejected_wrong_password"],
                serde_json::json!(true)
            );
            let hash = record["hash"].as_str().unwrap();
            let password = record["password"].as_str().unwrap();
            assert_eq!(check_password_hash(hash, password), Ok(true));
            assert_eq!(check_password_hash(hash, "wrong"), Ok(false));
        }
    }

    #[rstest]
    #[case::bare_scrypt("scrypt", HashMethod::DEFAULT)]
    #[case::bare_pbkdf2("pbkdf2", HashMethod::Pbkdf2Sha256 { iterations: DEFAULT_PBKDF2_ITERATIONS })]
    #[case::pbkdf2_sha256("pbkdf2:sha256", HashMethod::Pbkdf2Sha256 { iterations: DEFAULT_PBKDF2_ITERATIONS })]
    #[case::explicit("pbkdf2:sha256:600000", HashMethod::Pbkdf2Sha256 { iterations: 600_000 })]
    #[case::scrypt_full("scrypt:32768:8:1", HashMethod::Scrypt { n: 32768, r: 8, p: 1 })]
    fn parses_method_strings(#[case] raw: &str, #[case] expected: HashMethod) {
        assert_eq!(parse_method(raw), Ok(expected));
    }
}
