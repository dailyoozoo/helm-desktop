param(
  [ValidateRange(1, 100)]
  [int]$Runs = 5,
  [ValidateRange(30, 900)]
  [int]$CargoTimeoutSeconds = 300,
  [string]$CargoExecutable = 'cargo',
  [string[]]$CargoPrefixArguments = @(),
  [string]$CargoTargetDir
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repoRoot 'src-tauri/Cargo.toml'
$env:CARGO_TARGET_DIR = if ($CargoTargetDir) {
  [System.IO.Path]::GetFullPath($CargoTargetDir)
} else {
  Join-Path $repoRoot 'src-tauri/target'
}

$cases = [ordered]@{
  turn_start_transaction = @{
    args = @('--test', 'session_history', 'prepared_user_turn_rolls_back_all_history_side_effects_when_launch_is_rejected')
    baseline_p95_ms = 34556.70
  }
  session_restore = @{
    args = @('--test', 'session_history', 'session_history_persists_across_store_instances')
    baseline_p95_ms = 2025.82
  }
  stop_to_terminal = @{
    args = @('--lib', 'turn_supervisor::tests::persists_terminal_snapshot_for_restart_reconciliation')
    baseline_p95_ms = 3049.64
  }
  process_cleanup = @{
    args = @('--lib', 'adapter::tests::bounded_child_reap_falls_back_when_tree_kill_does_not_exit_target')
    baseline_p95_ms = 2009.51
  }
  history_rebuild = @{
    args = @('--test', 'session_history', 'change_27l_fresh_install_and_v21_upgrade_reopen_at_v30')
    baseline_p95_ms = 2537.73
  }
}

function Get-Percentile([double[]]$Values, [double]$Percentile) {
  $sorted = $Values | Sort-Object
  $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $sorted.Count) - 1)
  return [Math]::Round($sorted[$index], 2)
}

function Resolve-CargoInvocation([string[]]$Arguments) {
  $command = Get-Command $CargoExecutable -ErrorAction Stop | Select-Object -First 1
  if ($command.CommandType -eq [System.Management.Automation.CommandTypes]::ExternalScript) {
    $hostExecutable = (Get-Process -Id $PID).Path
    $quotedScript = '"' + $command.Source.Replace('"', '\"') + '"'
    return @{
      file_path = $hostExecutable
      arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $quotedScript) + $CargoPrefixArguments + $Arguments
    }
  }
  if ($command.CommandType -ne [System.Management.Automation.CommandTypes]::Application) {
    throw ('Unsupported Cargo command type: ' + $command.CommandType)
  }
  return @{ file_path = $command.Source; arguments = $CargoPrefixArguments + $Arguments }
}

function Invoke-Cargo([string[]]$Arguments) {
  try {
    $invocation = Resolve-CargoInvocation $Arguments
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $invocation.file_path
    $startInfo.Arguments = $invocation.arguments -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
      return @{ exit_code = -1; output = @(); timed_out = $false; started = $false }
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($CargoTimeoutSeconds * 1000)) {
      & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
      $process.WaitForExit(5000) | Out-Null
      return @{ exit_code = -1; output = @(); timed_out = $true; started = $true }
    }
    $process.WaitForExit()
    return @{
      exit_code = $process.ExitCode
      output = @(($stdoutTask.Result + [Environment]::NewLine + $stderrTask.Result) -split "`r?`n")
      timed_out = $false
      started = $true
    }
  } catch {
    return @{ exit_code = -1; output = @(); timed_out = $false; started = $false }
  }
}

$results = [ordered]@{}
$regressions = @()
foreach ($entry in $cases.GetEnumerator()) {
  $cargoArgs = @('test', '--manifest-path', $manifest) + $entry.Value.args + @('--', '--exact')
  $harness = $entry.Value.args[-1]
  $warmup = Invoke-Cargo $cargoArgs
  if (-not $warmup.started -or $warmup.timed_out -or $warmup.exit_code -ne 0 -or ($warmup.output -join [Environment]::NewLine) -notmatch '1 passed') {
    throw ('27L performance warmup did not pass exactly one test: ' + $harness + [Environment]::NewLine + $warmup.output)
  }
  $samples = @()
  for ($run = 1; $run -le $Runs; $run++) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $output = Invoke-Cargo $cargoArgs
    $watch.Stop()
    if (-not $output.started -or $output.timed_out -or $output.exit_code -ne 0 -or ($output.output -join [Environment]::NewLine) -notmatch '1 passed') {
      throw ('27L performance case failed or did not run exactly once: ' + $harness + [Environment]::NewLine + $output.output)
    }
    $samples += $watch.Elapsed.TotalMilliseconds
  }
  $p95 = Get-Percentile $samples 0.95
  $threshold = [Math]::Round(
    [Math]::Max($entry.Value.baseline_p95_ms * 1.25, $entry.Value.baseline_p95_ms + 500),
    2
  )
  $passed = $p95 -le $threshold
  if (-not $passed) {
    $regressions += ($entry.Key + ': p95=' + $p95 + 'ms > ' + $threshold + 'ms')
  }
  $results[$entry.Key] = [ordered]@{
    harness = $harness
    samples_ms = @($samples | ForEach-Object { [Math]::Round($_, 2) })
    p50_ms = Get-Percentile $samples 0.50
    p95_ms = $p95
    p99_ms = Get-Percentile $samples 0.99
    baseline_p95_ms = $entry.Value.baseline_p95_ms
    approved_threshold_ms = $threshold
    passed = $passed
    note = 'Test-process wall-clock time; threshold is the wider of 125% of 27A p95 or 27A p95 plus 500ms.'
  }
}

$report = [ordered]@{
  measured_at = (Get-Date).ToString('yyyy-MM-ddTHH:mm:sszzz')
  runs = $Runs
  rustc = (& rustc --version)
  cargo = (Invoke-Cargo @('--version')).output | Select-Object -First 1
  results = $results
}
$report | ConvertTo-Json -Depth 8
if ($regressions.Count -gt 0) {
  throw ('27L performance regression exceeded the approved threshold: ' + ($regressions -join '; '))
}
