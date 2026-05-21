# OTA — over-the-air firmware updates

## Goals

- New firmware can be pushed remotely without physical access.
- A bad firmware never bricks the device (automatic rollback on boot failure).
- Updates are tamper-resistant (Ed25519 signature verification before commit).

## Status

> TODO(phase-6): everything below is the planned design. Not implemented yet.

## Partition layout

See [`../partitions.csv`](../partitions.csv).

```
nvs            small KV (secrets, creds)
otadata        which slot to boot
phy_init       calibration
factory        recovery firmware (rarely updated)
ota_0          A slot (2 MB)
ota_1          B slot (2 MB)
storage        runtime KV (outbox, cache index, last config)
```

Boot order: `otadata` says which of `ota_0` / `ota_1` is the "active" slot. If both slots fail validation, fall back to `factory`.

## Flow

1. Sync task polls `GET /v1/firmware/latest` (endpoint TBD in `hush-protocol` phase 6).
2. Backend responds with `{ version, url, sha256, signature }`. If `version <= current`, no-op.
3. Sync task spawns the OTA worker (NOT in the audio path — preserve playback if running).
4. OTA worker:
   1. Downloads the binary into the inactive slot via streaming HTTPS.
   2. Verifies SHA-256 of the streamed bytes against the manifest.
   3. Verifies Ed25519 signature over `(version, sha256)` using the embedded public key.
   4. Writes the `otadata` marker `pending` for the inactive slot.
   5. Reboots.
5. On boot:
   1. Bootloader loads from the slot marked `pending`.
   2. New firmware runs its boot self-test (probe RFID, mount SD, contact backend).
   3. If self-test passes within 60 s, mark slot `valid` and clear `pending`.
   4. Else: bootloader notices `pending` flag stale on next boot, rolls back to previous valid slot.

## Public key embedding

The Ed25519 verification key is baked into the firmware at build time. It is the corresponding private key, held by the maintainer team, that signs releases. Rotation procedure TBD (probably: two embedded keys, sunset the old one in a release that's signed by both).

## What about the bootloader itself?

The ESP32-S3 bootloader is read-only ROM. We don't need to update it.

## Failure modes

| Failure | Detection | Recovery |
|---|---|---|
| Download interrupted | SHA-256 mismatch | Discard partial slot, retry on next sync |
| Signature mismatch | Ed25519 verify fails | Discard slot, log event, do **not** reboot |
| Boot self-test fails | 60 s timer not satisfied | Rollback on next boot |
| Both slots fail | Bootloader can't find a valid app | Fall back to `factory` (recovery firmware) |

## Open questions

> See `PLAN.md` § Decisions open.

- Should we offer signed delta patches (binary diff) for slow links? Probably not in v1.
- How big should the OTA download buffer be? 4 KB has worked elsewhere; confirm under TLS + decode pressure.
