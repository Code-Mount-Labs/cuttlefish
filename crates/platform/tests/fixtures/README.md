# Public cryptographic test fixture

These files encode the publicly documented Ed25519 test vector 1 from
[RFC 8032 section 7.1](https://www.rfc-editor.org/rfc/rfc8032.txt).
The PEM contains its published seed in PKCS#8 form; the text file is its
base64url public key. They are deterministic test data, not deployment keys.
Never configure a deployed service to use these fixtures.
