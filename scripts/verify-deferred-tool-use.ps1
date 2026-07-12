# Slice 2 审批机制验证脚本
# 目的：验证 Claude Code headless 模式是否输出 deferred_tool_use 事件
#
# 用法：
#   .\scripts\verify-deferred-tool-use.ps1
#
# 预期结果：
#   - 如果输出包含 "deferred_tool_use"，说明审批机制可用
#   - 如果不包含，说明需要采用 PTY 方案

param(
    [string]$ClaudeBin = "claude",
    [string]$TestDir = ".scratch\approval-test"
)

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Slice 2 审批机制验证脚本" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# 1. 准备测试环境
Write-Host "[1/5] 准备测试环境..." -ForegroundColor Yellow
if (Test-Path $TestDir) {
    Remove-Item -Recurse -Force $TestDir
}
New-Item -ItemType Directory -Path $TestDir | Out-Null

$hookDir = Join-Path $TestDir "hooks"
New-Item -ItemType Directory -Path $hookDir | Out-Null

$statePath = Join-Path $TestDir "approval-state.json"
$settingsPath = Join-Path $TestDir "settings.json"
$hookScriptPath = Join-Path $hookDir "approval-hook.ps1"
$outputPath = Join-Path $TestDir "claude-output.jsonl"

# 2. 写入 hook 脚本
Write-Host "[2/5] 创建审批 hook 脚本..." -ForegroundColor Yellow
$hookScript = @'
param([Parameter(Mandatory=$true)][string]$StatePath)

$raw = [Console]::In.ReadToEnd()
try {
  $payload = $raw | ConvertFrom-Json
} catch {
  $payload = $null
}

$toolName = ""
$requestId = ""
if ($payload -and $payload.tool_name) { $toolName = [string]$payload.tool_name }
if ($payload -and $payload.tool_use_id) { $requestId = [string]$payload.tool_use_id }

$decision = "defer"
$reason = "Helm approval test - deferred for UI approval"

# 只拦截 Write/Edit 工具
if ($toolName -match "^(Write|Edit|MultiEdit|NotebookEdit)$") {
  $decision = "defer"
} else {
  $decision = "allow"
}

$out = @{
  hookSpecificOutput = @{
    hookEventName = "PreToolUse"
    permissionDecision = $decision
    permissionDecisionReason = $reason
  }
}
$out | ConvertTo-Json -Depth 20 -Compress
'@
Set-Content -Path $hookScriptPath -Value $hookScript -Encoding UTF8

# 3. 写入 settings.json
Write-Host "[3/5] 创建 Claude 设置文件..." -ForegroundColor Yellow
$hookCommand = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$hookScriptPath`" `"$statePath`""
$settings = @{
    hooks = @{
        PreToolUse = @(
            @{
                matcher = "Write|Edit|MultiEdit|NotebookEdit"
                hooks = @(
                    @{
                        type = "shellCommand"
                        command = $hookCommand
                    }
                )
            }
        )
    }
} | ConvertTo-Json -Depth 10
Set-Content -Path $settingsPath -Value $settings -Encoding UTF8

# 4. 初始化状态文件
Write-Host "[4/5] 初始化审批状态文件..." -ForegroundColor Yellow
@{
    decisions = @{}
    alwaysAllow = @()
    deniedTargets = @()
} | ConvertTo-Json | Set-Content -Path $statePath -Encoding UTF8

# 5. 运行 Claude Code
Write-Host "[5/5] 启动 Claude Code headless 模式..." -ForegroundColor Yellow
Write-Host "命令：$ClaudeBin -p --output-format stream-json --verbose --include-hook-events --settings $settingsPath" -ForegroundColor Gray
Write-Host ""

$prompt = "Create a file named test.txt with content 'hello world'"

try {
    $process = Start-Process -FilePath "cmd" -ArgumentList "/C", "$ClaudeBin", "-p", "--output-format", "stream-json", "--verbose", "--include-hook-events", "--include-partial-messages", "--settings", $settingsPath, $prompt -NoNewWindow -Wait -PassThru -RedirectStandardOutput $outputPath -RedirectStandardError "$TestDir\stderr.log"

    Write-Host "✅ Claude Code 进程已完成（退出码: $($process.ExitCode)）" -ForegroundColor Green
    Write-Host ""
} catch {
    Write-Host "❌ 启动 Claude Code 失败：$_" -ForegroundColor Red
    exit 1
}

# 6. 分析输出
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "分析结果" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

if (-not (Test-Path $outputPath)) {
    Write-Host "❌ 输出文件不存在：$outputPath" -ForegroundColor Red
    exit 1
}

$output = Get-Content -Path $outputPath -Raw
$lines = Get-Content -Path $outputPath

Write-Host "📄 输出文件：$outputPath" -ForegroundColor Cyan
Write-Host "📏 总行数：$($lines.Count)" -ForegroundColor Cyan
Write-Host ""

# 检查关键字段
$hasDeferredToolUse = $output -match "deferred_tool_use"
$hasToolDeferred = $output -match '"stop_reason":\s*"tool_deferred"'
$hasHookEvents = $output -match "hook_started|hook_response"

Write-Host "关键字段检查：" -ForegroundColor Yellow
Write-Host "  - deferred_tool_use 字段：$(if ($hasDeferredToolUse) { '✅ 存在' } else { '❌ 不存在' })" -ForegroundColor $(if ($hasDeferredToolUse) { 'Green' } else { 'Red' })
Write-Host "  - stop_reason=tool_deferred：$(if ($hasToolDeferred) { '✅ 存在' } else { '❌ 不存在' })" -ForegroundColor $(if ($hasToolDeferred) { 'Green' } else { 'Red' })
Write-Host "  - hook 事件：$(if ($hasHookEvents) { '✅ 存在' } else { '❌ 不存在' })" -ForegroundColor $(if ($hasHookEvents) { 'Green' } else { 'Red' })
Write-Host ""

# 提取包含 deferred_tool_use 的行
if ($hasDeferredToolUse) {
    Write-Host "✅ 发现 deferred_tool_use 事件！" -ForegroundColor Green
    Write-Host ""
    Write-Host "相关行内容：" -ForegroundColor Yellow
    $lines | Where-Object { $_ -match "deferred_tool_use" } | ForEach-Object {
        $json = $_ | ConvertFrom-Json
        Write-Host ($json | ConvertTo-Json -Depth 5) -ForegroundColor White
    }
    Write-Host ""
    Write-Host "📋 结论：当前实现正确，审批机制应该可用" -ForegroundColor Green
    Write-Host "📋 下一步：检查为什么人工验收时未显示审批卡" -ForegroundColor Green
    Write-Host "📋 可能原因：配置遗漏、启动参数不一致、前端事件处理问题" -ForegroundColor Green
} else {
    Write-Host "❌ 未发现 deferred_tool_use 事件" -ForegroundColor Red
    Write-Host ""
    Write-Host "📋 结论：Claude Code headless 模式不支持 defer 审批" -ForegroundColor Red
    Write-Host "📋 下一步：实施 PTY 包装器方案（见 ADR 0004）" -ForegroundColor Red
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "完整输出已保存到：" -ForegroundColor Cyan
Write-Host "  - stdout: $outputPath" -ForegroundColor White
Write-Host "  - stderr: $TestDir\stderr.log" -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# 显示前 10 行输出供快速检查
Write-Host "前 10 行输出预览：" -ForegroundColor Yellow
$lines | Select-Object -First 10 | ForEach-Object {
    Write-Host $_ -ForegroundColor Gray
}

if ($lines.Count -gt 10) {
    Write-Host "... (共 $($lines.Count) 行，查看完整输出请打开上述文件)" -ForegroundColor Gray
}
