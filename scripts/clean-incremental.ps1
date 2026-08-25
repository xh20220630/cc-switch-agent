# CC Switch 开发缓存清理脚本
# 清理 target/debug/incremental(增量编译缓存),保留已编译产物。
# 下次编译会全量重编译但产物仍在,只失去增量加速;可每月执行一次。

$target = "D:\code_program\person\cc-switch\src-tauri\target\debug\incremental"

if (-not (Test-Path $target)) {
    Write-Host "增量缓存目录不存在,无需清理。"
    exit 0
}

$size = (Get-ChildItem $target -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
Remove-Item $target -Recurse -Force
Write-Host ("已清理增量缓存: {0:N2} GB" -f ($size / 1GB))
