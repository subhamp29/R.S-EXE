use sysinfo::Disks;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use serde::{Deserialize, Serialize};
use crate::commands::CommandResult;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Run a tokio::process::Command with a timeout to prevent indefinite hangs.
async fn run_command_with_timeout(
    cmd: &mut Command,
    context: &str,
) -> Result<std::process::Output, String> {
    timeout(COMMAND_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("{} timed out after {:?}", context, COMMAND_TIMEOUT))?
        .map_err(|e| format!("Failed to run {}: {}", context, e))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SystemInfo {
    pub total_ram_bytes: u64,
    pub free_ram_bytes: u64,
    pub cpu_cores: usize,
    pub cpu_model: String,
    pub platform: String,
    pub architecture: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub vram_bytes: u64,
    pub is_dedicated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HypervisorStatus {
    pub is_active: bool,
}

/// Result of a pre-operation disk space check.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskSpaceCheck {
    pub path: String,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub low_space: bool,       // under 5 GB free
    pub critical_space: bool,  // under 1 GB free
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_system_info() -> CommandResult<SystemInfo> {
    eprintln!("[DEBUG] get_system_info called");
    let platform = std::env::consts::OS.to_string();
    let architecture = std::env::consts::ARCH.to_string();

    #[cfg(target_os = "windows")]
    {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu();

        let total_ram = sys.total_memory();
        let free_ram = sys.available_memory();
        let cpu_cores = sys.cpus().len();
        let cpu_model = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        eprintln!("[DEBUG] get_system_info success: cpu={}, ram={}/{}", cpu_model, free_ram, total_ram);
        return CommandResult::success(SystemInfo {
            total_ram_bytes: total_ram,
            free_ram_bytes: free_ram,
            cpu_cores,
            cpu_model,
            platform,
            architecture,
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (platform, architecture);
        eprintln!("[DEBUG] get_system_info fail: not windows");
        CommandResult::fail(
            "System detection is not yet supported on this platform. Windows support is available now; other platforms coming soon.".to_string()
        )
    }
}

// ---------------------------------------------------------------------------
// GPU helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
async fn detect_gpus_windows() -> Result<Vec<GpuInfo>, String> {
    eprintln!("[DEBUG] detect_gpus_windows called");

    // Strategy 1: Try CIM (works on most Windows installs).
    if let Ok(gpus) = detect_gpus_via_cim().await {
        if !gpus.is_empty() {
            eprintln!("[DEBUG] detect_gpus_via_cim success: {} gpus", gpus.len());
            return Ok(gpus);
        }
    }
    eprintln!("[DEBUG] detect_gpus_via_cim failed or empty, trying WMI");

    // Strategy 2: Fall back to WMI (works on older Windows / restricted PS).
    if let Ok(gpus) = detect_gpus_via_wmi().await {
        if !gpus.is_empty() {
            eprintln!("[DEBUG] detect_gpus_via_wmi success: {} gpus", gpus.len());
            return Ok(gpus);
        }
    }
    eprintln!("[DEBUG] WMI also failed, trying registry");

    // Strategy 3: Fall back to registry enumeration.
    detect_gpus_via_registry().await
}

#[cfg(target_os = "windows")]
async fn detect_gpus_via_cim() -> Result<Vec<GpuInfo>, String> {
    // Strategy 1: Try CIM (works on most Windows installs).
    if let Ok(gpus) = detect_gpus_via_cim_instance().await {
        if !gpus.is_empty() {
            eprintln!("[DEBUG] detect_gpus_via_cim success: {} gpus", gpus.len());
            return Ok(gpus);
        }
    }
    eprintln!("[DEBUG] detect_gpus_via_cim failed or empty, trying WMI");

    // Strategy 2: Fall back to WMI (works on older Windows / restricted PS).
    if let Ok(gpus) = detect_gpus_via_wmi().await {
        if !gpus.is_empty() {
            eprintln!("[DEBUG] detect_gpus_via_wmi success: {} gpus", gpus.len());
            return Ok(gpus);
        }
    }
    eprintln!("[DEBUG] WMI also failed");
    Err("CIM and WMI GPU queries failed".to_string())
}

#[cfg(target_os = "windows")]
async fn detect_gpus_via_cim_instance() -> Result<Vec<GpuInfo>, String> {
    // This command has no `$` variables, so `-Command` is safe here.
    let ps_command = r#"Get-CimInstance Win32_VideoController | Select-Object Name, AdapterRAM, PNPDeviceID | ConvertTo-Json -Compress"#;

    let output = run_command_with_timeout(
        Command::new("powershell")
            .args(["-NoProfile", "-Command", ps_command]),
        "PowerShell GPU CIM query"
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("PowerShell GPU query failed: {}", stderr));
    }

    let stdout = decode_powershell_output(&output.stdout)?;
    if stdout.trim().is_empty() {
        return Ok(vec![]);
    }

    // FIX: ConvertTo-Json returns a single object (not an array) when there's
    // only one GPU. serde_json::from_str::<Vec<T>> fails on a bare object.
    // Parse as Value first, then normalize to an array.
    let raw_value: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse GPU JSON: {} — raw: {}", e, stdout))?;

    let raw_gpus: Vec<WindowsGpu> = match raw_value {
        serde_json::Value::Array(arr) => {
            serde_json::from_value(serde_json::Value::Array(arr))
                .map_err(|e| format!("Failed to parse GPU array: {} — raw: {}", e, stdout))?
        }
        serde_json::Value::Object(_) => {
            // Single GPU — wrap in an array
            vec![serde_json::from_value(raw_value)
                .map_err(|e| format!("Failed to parse single GPU object: {} — raw: {}", e, stdout))?]
        }
        _ => {
            return Err(format!("Unexpected GPU JSON format (expected object or array): {}", stdout));
        }
    };

    let mut gpus = Vec::with_capacity(raw_gpus.len());
    for g in raw_gpus {
        let is_dedicated = classify_gpu(
            g.pnp_device_id.as_deref().unwrap_or(""),
            g.name.as_deref().unwrap_or(""),
        );
        gpus.push(GpuInfo {
            name: g.name.unwrap_or_default(),
            vram_bytes: g.adapter_ram.unwrap_or(0),
            is_dedicated,
        });
    }

    Ok(gpus)
}

#[cfg(target_os = "windows")]
async fn detect_gpus_via_wmi() -> Result<Vec<GpuInfo>, String> {
    let ps_command = r#"Get-WmiObject Win32_VideoController | Select-Object Name, AdapterRAM, PNPDeviceID | ConvertTo-Json -Compress"#;

    let output = run_command_with_timeout(
        Command::new("powershell")
            .args(["-NoProfile", "-Command", ps_command]),
        "PowerShell GPU WMI query"
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("PowerShell WMI GPU query failed: {}", stderr));
    }

    let stdout = decode_powershell_output(&output.stdout)?;
    if stdout.trim().is_empty() {
        return Ok(vec![]);
    }

    let raw_value: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse WMI GPU JSON: {} — raw: {}", e, stdout))?;

    let raw_gpus: Vec<WindowsGpu> = match raw_value {
        serde_json::Value::Array(arr) => {
            serde_json::from_value(serde_json::Value::Array(arr))
                .map_err(|e| format!("Failed to parse WMI GPU array: {} — raw: {}", e, stdout))?
        }
        serde_json::Value::Object(_) => {
            vec![serde_json::from_value(raw_value)
                .map_err(|e| format!("Failed to parse single WMI GPU object: {} — raw: {}", e, stdout))?]
        }
        _ => {
            return Err(format!("Unexpected WMI GPU JSON format: {}", stdout));
        }
    };

    let mut gpus = Vec::with_capacity(raw_gpus.len());
    for g in raw_gpus {
        let is_dedicated = classify_gpu(
            g.pnp_device_id.as_deref().unwrap_or(""),
            g.name.as_deref().unwrap_or(""),
        );
        gpus.push(GpuInfo {
            name: g.name.unwrap_or_default(),
            vram_bytes: g.adapter_ram.unwrap_or(0),
            is_dedicated,
        });
    }

    Ok(gpus)
}

#[cfg(target_os = "windows")]
async fn detect_gpus_via_registry() -> Result<Vec<GpuInfo>, String> {
    eprintln!("[DEBUG] detect_gpus_via_registry started");
    let base_key = r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    // Step 1: List subkeys under the display driver class.
    let output = run_command_with_timeout(
        Command::new("reg")
            .args(["query", base_key]),
        "reg query display driver class"
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("[DEBUG] reg query failed: {}", stderr);
        return Err("reg query failed to list display driver subkeys".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("[DEBUG] reg query output: {}", stdout);
    let subkeys: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with(base_key) && line.len() > base_key.len() {
                let suffix = &line[base_key.len()..];
                let suffix = suffix.strip_prefix('\\').unwrap_or(suffix);
                if suffix.chars().all(|c| c.is_ascii_digit()) {
                    return Some(line.to_string());
                }
            }
            None
        })
        .collect();

    eprintln!("[DEBUG] found {} subkeys", subkeys.len());
    if subkeys.is_empty() {
        return Err("No display driver subkeys found in registry".to_string());
    }

    let mut gpus = Vec::with_capacity(subkeys.len());

    for subkey in subkeys {
        eprintln!("[DEBUG] querying subkey: {}", subkey);
        // Step 2: Query all values for each subkey (no /v filter) so that
        // missing individual values don't cause the whole query to fail.
        let ctx = format!("reg query {}", subkey);
        let output = run_command_with_timeout(
            Command::new("reg")
                .args(["query", &subkey]),
            ctx.as_str()
        ).await?;

        if !output.status.success() {
            eprintln!("[DEBUG] reg query for subkey failed");
            continue;
        }

        let out_str = String::from_utf8_lossy(&output.stdout);
        eprintln!("[DEBUG] subkey values: {}", out_str);
        let driver_desc = parse_reg_value(&out_str, "DriverDesc");
        let matching_device_id = parse_reg_value(&out_str, "MatchingDeviceId");

        let name = driver_desc.unwrap_or_default();
        if name.is_empty() {
            eprintln!("[DEBUG] skipping subkey with empty DriverDesc");
            continue;
        }

        // Try to get VRAM from registry values stored by the GPU driver.
        // SharedSystemMemory, DedicatedVideoMemory, DedicatedSystemMemory are
        // DWORD values (REG_DWORD) in hex, representing bytes.
        let vram_bytes = parse_reg_value(&out_str, "SharedSystemMemory")
            .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
            .or_else(|| {
                parse_reg_value(&out_str, "DedicatedVideoMemory")
                    .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
            })
            .or_else(|| {
                parse_reg_value(&out_str, "DedicatedSystemMemory")
                    .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
            });

        let is_dedicated = classify_gpu(
            matching_device_id.as_deref().unwrap_or(""),
            &name,
        );

        eprintln!("[DEBUG] found GPU: name={}, dedicated={}", name, is_dedicated);
        gpus.push(GpuInfo {
            name,
            vram_bytes: vram_bytes.unwrap_or(0),
            is_dedicated,
        });
    }

    if gpus.is_empty() {
        eprintln!("[DEBUG] no valid GPUs found");
        return Err("No GPUs with valid DriverDesc found in registry".to_string());
    }

    eprintln!("[DEBUG] detect_gpus_via_registry success: {} gpus", gpus.len());
    Ok(gpus)
}

/// Parse a `reg query` output line to extract a value's data.
///
/// `reg query` value output format:
///     ValueName    REG_TYPE    ValueData
fn parse_reg_value(output: &str, value_name: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(value_name) {
            // Find the REG_TYPE token (e.g., REG_SZ, REG_DWORD, REG_BINARY).
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                // ValueData starts right after the REG_TYPE token.
                if let Some(type_idx) = trimmed.find(parts[2]) {
                    let after_type = &trimmed[type_idx + parts[2].len()..];
                    let value = after_type.trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct WindowsGpu {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "AdapterRAM")]
    adapter_ram: Option<u64>,
    #[serde(rename = "PNPDeviceID")]
    pnp_device_id: Option<String>,
}

/// Classify a GPU as dedicated or integrated based on vendor ID + name heuristics.
fn classify_gpu(pnp_device_id: &str, name: &str) -> bool {
    // Parse vendor ID from PNPDeviceID: PCI\VEN_XXXX&...
    let vendor = pnp_device_id
        .split("VEN_")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .unwrap_or("");

    match vendor {
        "10DE" => true,   // NVIDIA — always dedicated
        "1002" => true,   // AMD — usually dedicated
        "8086" => false,  // Intel — usually integrated (Arc is rare)
        _ => {
            // Fallback: name-based heuristic when vendor ID is unknown.
            let upper = name.to_uppercase();
            // "UHD Graphics" and "Intel Iris" are Intel integrated GPUs
            // whose names don't always include the vendor string "INTEL".
            if upper.contains("UHD") || upper.contains("IRIS") {
                false
            } else if upper.contains("INTEL") {
                // Intel-branded but unknown specific model — assume integrated.
                false
            } else {
                // Unknown vendor with no clear integrated indicators — assume dedicated.
                true
            }
        }
    }
}

/// Decode PowerShell stdout which may be UTF-16LE (PS 5.1) or UTF-8 (PS 7+).
fn decode_powershell_output(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let data = if bytes.len() % 2 == 1 { &bytes[..bytes.len() - 1] } else { bytes };
        let utf16: Vec<u16> = data[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&utf16).map_err(|e| format!("UTF-16 decode error: {}", e))
    } else {
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Fetch VRAM (AdapterRAM) via .NET ManagementObjectSearcher.
///
/// This bypasses broken `Get-WmiObject`/`Get-CimInstance` cmdlets
/// by calling the underlying .NET WMI API directly from PowerShell.
/// Returns a map of GPU driver names to their VRAM bytes.
#[cfg(target_os = "windows")]
async fn fetch_gpu_vram_via_dotnet_wmi() -> Result<std::collections::HashMap<String, u64>, String> {
    let ps_command = r#"
$searcher = New-Object System.Management.ManagementObjectSearcher("SELECT Name, AdapterRAM FROM Win32_VideoController")
$results = @()
foreach ($obj in $searcher.Get()) {
    $results += @{Name=$obj.Name; AdapterRAM=$obj.AdapterRAM}
}
$results | ConvertTo-Json -Compress
"#;

    let output = run_command_with_timeout(
        Command::new("powershell")
            .args(["-NoProfile", "-Command", ps_command]),
        "PowerShell dotnet WMI VRAM query"
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("PowerShell dotnet WMI query failed: {}", stderr));
    }

    let stdout = decode_powershell_output(&output.stdout)?;
    if stdout.trim().is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let raw_value: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse dotnet WMI VRAM JSON: {} — raw: {}", e, stdout))?;

    let raw_gpus: Vec<DotnetWmiGpu> = match raw_value {
        serde_json::Value::Array(arr) => {
            serde_json::from_value(serde_json::Value::Array(arr))
                .map_err(|e| format!("Failed to parse dotnet WMI VRAM array: {} — raw: {}", e, stdout))?
        }
        serde_json::Value::Object(_) => {
            vec![serde_json::from_value(raw_value)
                .map_err(|e| format!("Failed to parse single dotnet WMI VRAM object: {} — raw: {}", e, stdout))?]
        }
        _ => {
            return Err(format!("Unexpected dotnet WMI VRAM JSON format: {}", stdout));
        }
    };

    let mut vram_map = std::collections::HashMap::new();
    for g in raw_gpus {
        if let Some(name) = g.name {
            let adapter_ram = g.adapter_ram.unwrap_or(0);
            eprintln!("[DEBUG] dotnet WMI VRAM: name={}, adapter_ram={}", name, adapter_ram);
            vram_map.insert(name, adapter_ram);
        }
    }

    Ok(vram_map)
}

#[derive(Debug, Deserialize)]
struct DotnetWmiGpu {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "AdapterRAM")]
    adapter_ram: Option<u64>,
}

#[tauri::command]
pub async fn detect_gpus() -> CommandResult<Vec<GpuInfo>> {
    #[cfg(target_os = "windows")]
    {
        match detect_gpus_windows().await {
            Ok(gpus) => CommandResult::success(gpus),
            Err(e) => CommandResult::fail(e),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        CommandResult::fail(
            "GPU detection is not yet supported on this platform. Windows support is available now; other platforms coming soon.".to_string()
        )
    }
}

// ---------------------------------------------------------------------------
// Hypervisor helpers
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn check_hypervisor() -> CommandResult<HypervisorStatus> {
    #[cfg(target_os = "windows")]
    {
        match check_hypervisor_windows().await {
            Ok(status) => CommandResult::success(status),
            Err(e) => CommandResult::fail(e),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        CommandResult::fail(
            "Hypervisor detection is not yet supported on this platform. Windows support is available now; other platforms coming soon.".to_string()
        )
    }
}

#[cfg(target_os = "windows")]
async fn check_hypervisor_windows() -> Result<HypervisorStatus, String> {
    eprintln!("[DEBUG] check_hypervisor_windows called");
    // Strategy 1: Try WMIC first (works on most systems).
    if let Ok(status) = check_hypervisor_via_wmic().await {
        eprintln!("[DEBUG] check_hypervisor_via_wmic success: active={}", status.is_active);
        return Ok(status);
    }
    eprintln!("[DEBUG] check_hypervisor_via_wmic failed, trying Windows features");

    // Strategy 2: Fall back to Windows Features check.
    check_hypervisor_via_windows_features().await
}

#[cfg(target_os = "windows")]
async fn check_hypervisor_via_wmic() -> Result<HypervisorStatus, String> {
    let output = run_command_with_timeout(
        Command::new("wmic")
            .args(["computersystem", "get", "HypervisorPresent", "/value"]),
        "wmic hypervisor check"
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("wmic hypervisor query failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let is_active = stdout.contains("TRUE");

    Ok(HypervisorStatus { is_active })
}

#[cfg(target_os = "windows")]
async fn check_hypervisor_via_windows_features() -> Result<HypervisorStatus, String> {
    eprintln!("[DEBUG] check_hypervisor_via_windows_features started");
    // Check hypervisor status using two independent sources:
    // 1. bcdedit: hypervisorlaunchtype must be Auto
    // 2. DISM: HypervisorPlatform feature must be Enabled
    let bcd_output = run_command_with_timeout(
        Command::new("bcdedit")
            .args(["/enum"]),
        "bcdedit"
    ).await?;

    if !bcd_output.status.success() {
        let stderr = String::from_utf8_lossy(&bcd_output.stderr);
        eprintln!("[DEBUG] bcdedit failed: {}", stderr);
        return Err(format!("bcdenum failed: {}", stderr));
    }

    let bcd_stdout = String::from_utf8_lossy(&bcd_output.stdout);
    let launch_auto = bcd_stdout.contains("hypervisorlaunchtype") && bcd_stdout.contains("Auto");
    eprintln!("[DEBUG] bcdedit launch_auto={}", launch_auto);

    let dism_output = run_command_with_timeout(
        Command::new("dism")
            .args(["/online", "/Get-FeatureInfo", "/FeatureName:HypervisorPlatform"]),
        "DISM feature check"
    ).await?;

    if !dism_output.status.success() {
        let stderr = String::from_utf8_lossy(&dism_output.stderr);
        eprintln!("[DEBUG] dism failed: {}", stderr);
        return Err(format!("DISM feature check failed: {}", stderr));
    }

    let dism_stdout = String::from_utf8_lossy(&dism_output.stdout);
    let feature_enabled = dism_stdout.contains("State : Enabled");
    eprintln!("[DEBUG] dism feature_enabled={}", feature_enabled);

    let is_active = launch_auto && feature_enabled;
    eprintln!("[DEBUG] hypervisor is_active={}", is_active);

    Ok(HypervisorStatus { is_active })
}

// ---------------------------------------------------------------------------
// App version
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_app_version() -> CommandResult<String> {
    CommandResult::success(env!("CARGO_PKG_VERSION").to_string())
}

// ---------------------------------------------------------------------------
// Phase 7 — Disk space check
// ---------------------------------------------------------------------------

/// Check available disk space on the drive containing `path`.
///
/// Returns a [`DiskSpaceCheck`] with `low_space` (under 5 GB) and
/// `critical_space` (under 1 GB) flags so the frontend can warn
/// or block the operation using the same modal pattern as the
/// Phase 5 resource-aware boot warning.
#[tauri::command]
pub fn check_disk_space(path: String) -> CommandResult<DiskSpaceCheck> {
    let path_buf = std::path::PathBuf::from(&path);
    let mount_point = if cfg!(windows) {
        // On Windows, find the drive root for the given path.
        path_buf
            .ancestors()
            .filter(|a| a.as_os_str().len() == 3 && a.as_os_str().to_string_lossy().ends_with(':'))
            .next()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("C:\\"))
    } else {
        // On Unix, find the mount point by checking which ancestor is a mount point.
        // A simple heuristic: use the path itself if it exists, or fall back to "/".
        if path_buf.exists() {
            path_buf.clone()
        } else {
            std::path::PathBuf::from("/")
        }
    };

    let disks = Disks::new_with_refreshed_list();
    let disk = disks.list().iter().find(|d| {
        d.mount_point() == mount_point
            || path_buf.starts_with(d.mount_point())
    });

    let (available_bytes, total_bytes) = if let Some(d) = disk {
        (d.available_space(), d.total_space())
    } else {
        // Fallback: try the path directly.
        let metadata = std::fs::metadata(&path_buf).ok();
        let available = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        (available, 0)
    };

    let low_space = available_bytes < 5 * 1024 * 1024 * 1024;    // under 5 GB
    let critical_space = available_bytes < 1 * 1024 * 1024 * 1024; // under 1 GB

    eprintln!(
        "[DISK] check_disk_space: path='{}' mount='{}' available={}GB total={}GB low={} critical={}",
        path,
        mount_point.display(),
        available_bytes / 1024 / 1024 / 1024,
        total_bytes / 1024 / 1024 / 1024,
        low_space,
        critical_space,
    );

    CommandResult::success(DiskSpaceCheck {
        path,
        available_bytes,
        total_bytes,
        low_space,
        critical_space,
    })
}
