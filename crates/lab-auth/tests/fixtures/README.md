# Synthetic RS256 compatibility fixture

This private key is PUBLIC TEST DATA, generated only for this test and never used by an account or service. The token was signed using OpenSSL SHA-256/PKCS#1 v1.5 independently of jsonwebtoken; claims expire in 2100. It verifies standard RS256 wire compatibility across JWT backends. Never use this key outside tests.

## Backend compatibility boundary

The AWS-LC RS256 verifier used by jsonwebtoken accepts RSA moduli from 2048 through 8192 bits. lab-auth generates 2048-bit keys, so its generated persisted keys remain supported. Externally supplied older keys below 2048 bits (or above 8192 bits) may have worked with RustCrypto but are outside this backend's verification range. Before rollout, inspect public key sizes without logging private key material; do not silently rotate or overwrite an existing key. Unsupported keys require an explicit key migration, with token/session impact planned by the operator.

The fixture regression verifies an existing 2048-bit PKCS8 key and an independently signed token, preserves its key ID, does not rewrite the key, and reproduces exactly the same RS256 token. Separate tests reject modified signatures, expired tokens and a valid HMAC token with the wrong algorithm.
