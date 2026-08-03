param(
  [Parameter(Mandatory = $true)]
  [string]$AppConfigDir,
  [string]$OutputPath,
  [ValidateRange(1, 120)]
  [int]$StatusTimeoutSeconds = 15
)

$ErrorActionPreference = 'Stop'
$appConfig = [System.IO.Path]::GetFullPath($AppConfigDir)
$claudeProfile = Join-Path $appConfig 'cli-profiles/claude-subscription'
$codexProfile = Join-Path $appConfig 'cli-profiles/codex-subscription'

function Get-CliVersion([string]$Executable) {
  $result = Invoke-BoundedProcess $Executable @('--version') $null $null $StatusTimeoutSeconds
  if ($result.timed_out) { return 'timeout' }
  if (-not $result.started) { return 'unavailable' }
  if ($result.exit_code -ne 0) { return 'unavailable' }
  return (($result.output -split "`r?`n" | Select-Object -First 1) -as [string]).Trim()
}

function Resolve-ProcessInvocation([string]$Executable, [string[]]$Arguments) {
  $command = Get-Command $Executable -ErrorAction Stop | Select-Object -First 1
  if ($command.CommandType -eq [System.Management.Automation.CommandTypes]::ExternalScript) {
    $hostExecutable = (Get-Process -Id $PID).Path
    $quotedScript = '"' + $command.Source.Replace('"', '\"') + '"'
    return @{
      file_path = $hostExecutable
      arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $quotedScript) + $Arguments
    }
  }
  if ($command.CommandType -ne [System.Management.Automation.CommandTypes]::Application) {
    throw ('Unsupported CLI command type: ' + $command.CommandType)
  }
  return @{ file_path = $command.Source; arguments = $Arguments }
}

function Invoke-BoundedProcessOnce(
  [string]$Executable,
  [string[]]$Arguments,
  [string]$EnvironmentName,
  [string]$Profile,
  [int]$TimeoutSeconds
) {
  $previous = if ($EnvironmentName) {
    [Environment]::GetEnvironmentVariable($EnvironmentName, 'Process')
  } else {
    $null
  }
  try {
    if ($EnvironmentName) {
      [Environment]::SetEnvironmentVariable($EnvironmentName, $Profile, 'Process')
    }
    $invocation = Resolve-ProcessInvocation $Executable $Arguments
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
      return @{ exit_code = -1; output = ''; timed_out = $false; started = $false }
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
      & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
      $process.WaitForExit(5000) | Out-Null
      return @{ exit_code = -1; output = ''; timed_out = $true; started = $true }
    }
    $process.WaitForExit()
    $combined = ($stdoutTask.Result + ' ' + $stderrTask.Result).ToLowerInvariant()
    return @{ exit_code = $process.ExitCode; output = $combined; timed_out = $false; started = $true }
  } catch {
    return @{ exit_code = -1; output = ''; timed_out = $false; started = $false }
  } finally {
    if ($EnvironmentName) {
      [Environment]::SetEnvironmentVariable($EnvironmentName, $previous, 'Process')
    }
  }
}

function Invoke-BoundedProcess(
  [string]$Executable,
  [string[]]$Arguments,
  [string]$EnvironmentName,
  [string]$Profile,
  [int]$TimeoutSeconds
) {
  $result = Invoke-BoundedProcessOnce $Executable $Arguments $EnvironmentName $Profile $TimeoutSeconds
  if (-not $result.started -and -not $result.timed_out) {
    Start-Sleep -Milliseconds 100
    return Invoke-BoundedProcessOnce $Executable $Arguments $EnvironmentName $Profile $TimeoutSeconds
  }
  return $result
}

$claudeStatus = Invoke-BoundedProcess 'claude' @('auth', 'status') 'CLAUDE_CONFIG_DIR' $claudeProfile $StatusTimeoutSeconds
$claudeAuth = 'unknown'
if (-not $claudeStatus.started) {
  $claudeAuth = 'unavailable'
} elseif ($claudeStatus.timed_out) {
  $claudeAuth = 'timeout'
} elseif ($claudeStatus.exit_code -ne 0) {
  $claudeAuth = 'missing'
} else {
  try {
    $parsed = $claudeStatus.output | ConvertFrom-Json
    if (-not $parsed.loggedIn) {
      $claudeAuth = 'missing'
    } elseif (($parsed.authMethod -as [string]).ToLowerInvariant().Contains('oauth')) {
      $claudeAuth = 'subscription'
    } elseif (($parsed.authMethod -as [string]).ToLowerInvariant().Contains('api')) {
      $claudeAuth = 'apikey'
    }
  } catch {
    $claudeAuth = 'unknown'
  }
}

$codexStatus = Invoke-BoundedProcess 'codex' @('login', 'status') 'CODEX_HOME' $codexProfile $StatusTimeoutSeconds
$codexAuth = if (-not $codexStatus.started) {
  'unavailable'
} elseif ($codexStatus.timed_out) {
  'timeout'
} elseif ($codexStatus.exit_code -ne 0) {
  'missing'
} elseif ($codexStatus.output.Contains('chatgpt')) {
  'subscription'
} elseif ($codexStatus.output.Contains('api key')) {
  'apikey'
} else {
  'unknown'
}

# A timed-out authority check already proves this CLI is not release-ready.
# Do not launch a second process for --version: serial timeout + taskkill costs
# otherwise make the audit's own hard bound nondeterministic on Windows.
$claudeVersion = if ($claudeStatus.timed_out) { 'timeout' } else { Get-CliVersion 'claude' }
$codexVersion = if ($codexStatus.timed_out) { 'timeout' } else { Get-CliVersion 'codex' }

$manualCases = @(
  'Claude subscription/API: text, tool, approval, deny, stop, resume',
  'Codex subscription/API: text, item, ServerRequest, deny, stop, resume',
  'Same-provider model switch; API/subscription Binding switch; Codex app-server rebuild',
  'Claude native resume/Ledger rebuild; Codex thread resume/Ledger rebuild',
  'Claude-to-Codex and Codex-to-Claude Handoff/SessionFork',
  'API/subscription automatic title, Operation owner, and no-tools launch evidence for both engines',
  'Process tree, Profile, temporary directory, app-server task, and CODEX_HOME cleanup',
  'Windows Tauri permissions, model badge, Binding, Context, history, derivation, Stop/approval, and viewports'
)

$report = [ordered]@{
  captured_at = (Get-Date).ToString('yyyy-MM-ddTHH:mm:sszzz')
  app_config_dir = $appConfig
  environment = [ordered]@{
    windows = [Environment]::OSVersion.VersionString
    node = (& node --version)
    npm = (& npm --version)
    rustc = (& rustc --version)
    cargo = (& cargo --version)
  }
  engines = [ordered]@{
    claude = [ordered]@{
      version = $claudeVersion
      isolated_profile_exists = Test-Path -LiteralPath $claudeProfile
      auth_method = $claudeAuth
    }
    codex = [ordered]@{
      version = $codexVersion
      isolated_profile_exists = Test-Path -LiteralPath $codexProfile
      auth_method = $codexAuth
    }
  }
  evidence_policy = 'Record protocol reachability, model success, failure classification, and manual UI separately; a handshake is not model success.'
  manual_cases = $manualCases
}

$json = $report | ConvertTo-Json -Depth 8
if ($OutputPath) {
  $resolvedOutput = [System.IO.Path]::GetFullPath($OutputPath)
  [System.IO.File]::WriteAllText($resolvedOutput, $json, [System.Text.UTF8Encoding]::new($false))
}
$json
