# apexd D-Bus contract (FROZEN at M3)

`apexd` exposes a single service on the **system bus**. This document is the
frozen interface contract: the apex-shell `PowerProfileService`, the `apex`
CLI, and any Grafana/metrics consumers depend on it. Changes after M3 are
additive only (new members / new interfaces); existing signatures and the
tier IDs never change.

- **Bus name:** `org.apexos.Apexd1`
- **Object path:** `/org/apexos/Apexd1`
- All six interfaces live on that one object path.

## Tier IDs (frozen)

Exactly these five strings, ordered most→least aggressive. They are the value
of `.Power.Tier`, the argument to `SetTier`, and the members of `.Power.Tiers`:

```
ultra-max   ultra   performance   balanced   power-saver
```

They match the apex-shell picker IDs verbatim (`PowerProfileService.qml`).

## `org.apexos.Apexd1.Power`

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `Tier` | property (r) | `s` | Current tier ID |
| `Tiers` | property (r) | `as` | All tier IDs, high→low |
| `OnAcPower` | property (r) | `b` | AC line online |
| `AutoSwitch` | property (r) | `b` | AC/battery auto-switching enabled |
| `SetTier` | method | `s → ()` | Switch tier; `InvalidArgs` on unknown ID; polkit `manage-power` |
| `SetAutoSwitch` | method | `b → ()` | Toggle auto-switch; enabling reconciles immediately; polkit `manage-power` |
| `TierChanged` | signal | `s` | Emitted whenever the active tier changes |

## `org.apexos.Apexd1.Battery`

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `ChargeStart` | property (r) | `y` | Charge start threshold (%) |
| `ChargeEnd` | property (r) | `y` | Charge stop threshold (%) |
| `TravelMode` | property (r) | `b` | Travel/storage window active |
| `Capacity` | property (r) | `y` | Battery charge (%) |
| `Status` | property (r) | `s` | e.g. `Charging`, `Discharging`, `Full` |
| `SetChargeThresholds` | method | `yy → ()` | start, end; `InvalidArgs` if start>end or end>100; polkit `manage-battery` |
| `SetTravelMode` | method | `b → ()` | On = 55/60 storage window; off = restore profile defaults; polkit `manage-battery` |
| `Calibrate` | method | `() → ()` | Opens a 0/100 calibration window; polkit `manage-battery` |

## `org.apexos.Apexd1.Profile` (read-only)

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `Active` | property (r) | `s` | Effective profile ID (device ?? class ?? generic) |
| `Class` | property (r) | `s` | CPU-class profile ID, or `""` |
| `Device` | property (r) | `s` | Exact-device profile ID, or `""` |

## `org.apexos.Apexd1.Metrics`

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `Snapshot` | property (r) | `a{sv}` | Best-effort telemetry: `tier`(s), `on_ac`(b), `ppt_watts`(d), `battery_uwh`(t), `temp_<zone>`(d) |

## `org.apexos.Apexd1.Fan` (stub — real impl M6)

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `Mode` | property (r) | `s` | Always `auto` for now |
| `SetMode` | method | `s → ()` | No-op stub |

## `org.apexos.Apexd1.GameMode` (stub — real impl M6)

| Member | Kind | Signature | Notes |
|---|---|---|---|
| `Active` | property (r) | `b` | Always `false` for now |
| `SetActive` | method | `b → ()` | No-op stub |

## Authorization

The D-Bus system policy (`org.apexos.Apexd1.conf`) lets only root own the name
and lets any local user *send* to the service. Reads are unrestricted;
**mutating methods are gated by polkit inside the daemon**:

- `SetTier`, `SetAutoSwitch` → action `org.apexos.apexd.manage-power`
- `SetChargeThresholds`, `SetTravelMode`, `Calibrate` → action `org.apexos.apexd.manage-battery`

Both actions ship `allow_active = yes` (the logged-in local user acts
**passwordless**), `allow_inactive`/`allow_any = auth_admin`. The daemon calls
`org.freedesktop.PolicyKit1.Authority.CheckAuthorization` with the caller's
`system-bus-name` and **fails closed** if polkit is unreachable.

## Metrics HTTP endpoint

Prometheus text exposition on `http://127.0.0.1:9723/metrics` (any path):

```
apexd_tier{tier="ultra-max|ultra|performance|balanced|power-saver"}  0|1
apexd_ac_online                                                      0|1
apexd_ppt_watts                          <watts>        (if a hwmon source exists)
apexd_battery_uwh                        <microwatt-hours>  (if BAT*/energy_now exists)
apexd_temp_celsius{zone="<thermal-zone-type>"}  <celsius>  (per thermal zone)
```

All metric sources are best-effort and read-only; a missing source omits its
line rather than erroring.

## Install paths (image)

| File | Installed to |
|---|---|
| `apexd/apexd/apexd.service` | `/usr/lib/systemd/system/apexd.service` |
| `files/system/dbus-1/system.d/org.apexos.Apexd1.conf` | `/usr/share/dbus-1/system.d/` |
| `files/system/dbus-1/system-services/org.apexos.Apexd1.service` | `/usr/share/dbus-1/system-services/` |
| `files/system/polkit-1/actions/org.apexos.apexd.policy` | `/usr/share/polkit-1/actions/` |
| `config/sysprofiles/*.toml` | `/usr/share/apexos/sysprofiles/` (overrides the embedded set) |
| `apexd`, `apex` binaries | `/usr/bin/` |
