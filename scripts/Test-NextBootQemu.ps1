[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(0, 999)]
    [int] $DiskNumber,

    [ValidateRange(512, 65536)]
    [int] $MemoryMiB = 4096,

    [string] $QemuPath = 'C:\Program Files\qemu\qemu-system-x86_64.exe',

    [string] $OvmfCodePath = 'C:\Program Files\qemu\share\edk2-x86_64-code.fd',

    [string] $OvmfVarsTemplatePath = 'C:\Program Files\qemu\share\edk2-i386-vars.fd',

    [string] $ArtifactDirectory = (Join-Path $PSScriptRoot '..\target\qemu-physical')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run PowerShell as Administrator so QEMU can read the selected PhysicalDrive.'
    }
}

Require-Administrator

foreach ($path in @($QemuPath, $OvmfCodePath, $OvmfVarsTemplatePath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required QEMU or OVMF file was not found: $path"
    }
}

$disk = Get-Disk -Number $DiskNumber -ErrorAction Stop
if ($disk.IsBoot -or $disk.IsSystem) {
    throw "Refusing to start QEMU with Windows system or boot disk $DiskNumber."
}
if ($disk.Size -lt 8GB) {
    throw "Disk $DiskNumber is smaller than the 8 GB minimum NextBoot media size."
}

$artifactRoot = [IO.Path]::GetFullPath($ArtifactDirectory)
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$vars = Join-Path $artifactRoot "OVMF_VARS_Disk$DiskNumber.fd"
$serialLog = Join-Path $artifactRoot "Disk$DiskNumber.serial.log"
Copy-Item -LiteralPath $OvmfVarsTemplatePath -Destination $vars -Force
Remove-Item -LiteralPath $serialLog -Force -ErrorAction SilentlyContinue

$physicalDrive = "\\.\PhysicalDrive$DiskNumber"
$arguments = @(
    '-machine', 'q35,accel=tcg',
    '-m', "$MemoryMiB",
    '-smp', '4',
    '-drive', "if=pflash,format=raw,readonly=on,file=$OvmfCodePath",
    '-drive', "if=pflash,format=raw,file=$vars",
    '-drive', "id=nextboot,file=$physicalDrive,format=raw,if=none,cache=none",
    '-device', 'virtio-blk-pci,drive=nextboot,bootindex=0',
    # QEMU keeps all guest writes in a temporary overlay. The selected
    # PhysicalDrive is never modified by this test.
    '-snapshot',
    '-display', 'gtk',
    '-serial', "file:$serialLog"
)

Write-Host "Starting read-only QEMU snapshot test for Disk $DiskNumber ($($disk.FriendlyName), $([math]::Floor($disk.Size / 1GB)) GB)."
Write-Host 'Close the QEMU window to finish. Guest writes are discarded because -snapshot is enabled.'
& $QemuPath @arguments
$exitCode = $LASTEXITCODE

if (Test-Path -LiteralPath $serialLog) {
    Write-Host "Serial log: $serialLog"
    Get-Content -LiteralPath $serialLog -Tail 120
}
if ($exitCode -ne 0) {
    throw "QEMU exited with code $exitCode. Review the serial log above."
}
