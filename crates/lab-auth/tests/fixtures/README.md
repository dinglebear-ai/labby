# Public RS256 compatibility fixture

The public key and token are PUBLIC TEST DATA generated solely for this test, never an account or service. The token was independently signed using OpenSSL SHA-256/PKCS#1 v1.5; example.com claims expire in 2100. Only the public verification key and signed token remain in the repository. Tests generate private keys at runtime in temporary directories and never require a committed private key.

## Backend compatibility boundary

The AWS-LC RS256 verifier used by jsonwebtoken accepts RSA moduli from 2048 through 8192 bits. lab-auth generates 2048-bit keys, so its generated persisted keys remain supported. Externally supplied older keys below 2048 bits (or above 8192 bits) may have worked with RustCrypto but are outside this backend's verification range. Before rollout, inspect public key sizes without logging private key material; do not silently rotate or overwrite an existing key. Unsupported keys require an explicit key migration, with token/session impact planned by the operator.

The public fixture validates an independently signed preexisting RS256 token. Runtime-generated private keys separately exercise persistence, key-ID stability and byte-identical RS256 output after reload. Negative cases reject modified signatures, expired tokens and a valid HMAC token with the wrong algorithm.

## Provenance

Commit 66d1ca11 originally included the freshly generated synthetic private counterpart, clearly labeled test-only; it was never installed or used by a service. It was removed from the branch tip after generic secret scanning flagged it. Removing it does not erase Git history or resolve the historical alert; alert disposition requires authorized review. The public-key DER SHA-256 fingerprint is 6b96dc0e522c25624900fcae5812f69adc4e6a510f0f5e3dbd98ed54cbc9e37c.
