# Test-Schlüssel

`oidc-test-key.pem` und `oidc-other-key.pem` sind RSA-Schlüssel, die ausschließlich für die OIDC-Tests erzeugt wurden (`openssl genpkey -algorithm RSA`). Sie schützen nichts und dürfen nirgends sonst verwendet werden. Die `*.jwk.json`-Dateien enthalten die zugehörigen öffentlichen Schlüssel im JWK-Format.
