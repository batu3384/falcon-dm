Status: DONE

Commits:
- c8696d201af7e7e2c33f95e55bb1172154eb6470: feat: aria2 sidecar integration with RPC client

Tests:
- `cargo check` passed successfully.

Concerns:
- aria2c binary is currently not bundled. The engine will gracefully ignore failure if the executable is not available on the path. We will need to bundle it in future steps.
