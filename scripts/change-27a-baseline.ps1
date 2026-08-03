param(
  [ValidateRange(5, 100)]
  [int]$Runs = 5
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repoRoot 'src-tauri/Cargo.toml'
$env:CARGO_TARGET_DIR = Join-Path $repoRoot 'target/change-27a-baseline'

$cases = [ordered]@{
  turn_start_transaction = @('--test', 'session_history', 'prepared_user_turn_rolls_back_all_history_side_effects_when_launch_is_rejected')
  session_restore = @('--test', 'session_history', 'session_history_persists_across_store_instances')
  stop_to_terminal = @('--lib', 'turn_supervisor::tests::persists_terminal_snapshot_for_restart_reconciliation')
  process_cleanup = @('--lib', 'adapter::tests::bounded_child_reap_falls_back_when_tree_kill_does_not_exit_target')
  history_rebuild = @('--test', 'session_history', 'schema_v19_migrates_to_v21_without_losing_sessions')
}

function Get-Percentile([double[]]$Values, [double]$Percentile) {
  $sorted = $Values | Sort-Object
  $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $sorted.Count) - 1)
  return [Math]::Round($sorted[$index], 2)
}

$results = [ordered]@{}
foreach ($entry in $cases.GetEnumerator()) {
  $cargoArgs = @('test', '--manifest-path', $manifest) + $entry.Value + @('--', '--exact')
  $harness = $entry.Value[-1]
  $warmup = & cargo @cargoArgs 2>&1
  if ($LASTEXITCODE -ne 0 -or ($warmup -join "`n") -notmatch '1 passed') {
    throw "基线预热没有准确通过 1 个测试：$harness`n$warmup"
  }
  $samples = @()
  for ($run = 1; $run -le $Runs; $run++) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $output = & cargo @cargoArgs 2>&1
    if ($LASTEXITCODE -ne 0) {
      throw "基线用例失败：$harness`n$output"
    }
    if (($output -join "`n") -notmatch '1 passed') {
      throw "基线用例没有准确执行 1 个测试：$harness`n$output"
    }
    $watch.Stop()
    $samples += $watch.Elapsed.TotalMilliseconds
  }
  $results[$entry.Key] = [ordered]@{
    harness = $harness
    samples_ms = @($samples | ForEach-Object { [Math]::Round($_, 2) })
    p50_ms = Get-Percentile $samples 0.50
    p95_ms = Get-Percentile $samples 0.95
    p99_ms = Get-Percentile $samples 0.99
    note = '端到端测试进程墙钟时间，包含测试二进制启动；用于后续同机回归比较，不等同 UI 延迟。'
  }
}

[ordered]@{
  measured_at = (Get-Date).ToString('yyyy-MM-ddTHH:mm:sszzz')
  runs = $Runs
  rustc = (& rustc --version)
  cargo = (& cargo --version)
  results = $results
} | ConvertTo-Json -Depth 8
