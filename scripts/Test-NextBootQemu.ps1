[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(0, 999)]
    [int] $DiskNumber,

    [ValidateRange(512, 65536)]
    [int] $MemoryMiB = 4096,

    # When supplied, require this exact /ISO file to appear in the NextBoot
    # menu log before the preflight is considered successful.
    [string] $ExpectedImageName,

    # Reproduce the common desktop topology: the NextBoot medium plus two
    # separate fixed disks. This exercises the scanner's boot-media filter.
    [ValidateRange(0, 8)]
    [int] $SyntheticInternalDiskCount = 2,

    [ValidateRange(16, 4096)]
    [int] $SyntheticInternalDiskSizeMiB = 128,

    # Run without a GUI and stop QEMU as soon as the serial log proves that
    # NextBoot reached its menu. This avoids booting the selected ISO merely
    # to perform a local preflight.
    [switch] $Headless,

    [ValidateRange(5, 600)]
    [int] $PreflightTimeoutSeconds = 90,

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

function Get-MissingPreflightMarkers([string] $LogText, [int] $AttachedDiskCount, [string] $ImageName) {
    $requiredMarkers = @(
        'NextBoot v',
        'Phase 1: Detecting storage devices',
        'Phase 2: Scanning for ISO files',
        'Phase 3: Displaying boot menu'
    )
    if ($AttachedDiskCount -gt 0) {
        $requiredMarkers += "Found $($AttachedDiskCount + 1) storage device(s)"
    }
    if (-not [string]::IsNullOrWhiteSpace($ImageName)) {
        $requiredMarkers += "/ISO/$ImageName"
    }
    return @($requiredMarkers | Where-Object { -not $LogText.Contains($_) })
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
    '-display', $(if ($Headless) { 'none' } else { 'gtk' }),
    '-serial', "file:$serialLog"
)

for ($index = 1; $index -le $SyntheticInternalDiskCount; $index++) {
    $syntheticDisk = Join-Path $artifactRoot "synthetic-internal-$index.raw"
    $stream = [IO.File]::Open($syntheticDisk, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try {
        $stream.SetLength([Int64]$SyntheticInternalDiskSizeMiB * 1MB)
    }
    finally {
        $stream.Dispose()
    }
    $driveId = "synthetic$index"
    $arguments += @(
        '-drive', "id=$driveId,file=$syntheticDisk,format=raw,if=none",
        '-device', "virtio-blk-pci,drive=$driveId,serial=NEXTBOOTTEST$index"
    )
}

Write-Host "Starting read-only QEMU snapshot test for Disk $DiskNumber ($($disk.FriendlyName), $([math]::Floor($disk.Size / 1GB)) GB)."
if ($Headless) {
    Write-Host "Headless preflight will stop automatically after the menu markers are observed (timeout: $PreflightTimeoutSeconds seconds)."
}
else {
    Write-Host 'Close the QEMU window to finish. Guest writes are discarded because -snapshot is enabled.'
}
if ($SyntheticInternalDiskCount -gt 0) {
    Write-Host "Attached $SyntheticInternalDiskCount temporary fixed disks to exercise the boot-media scan filter."
}
if ($Headless) {
    # Start-Process accepts one command-line string. Quote every QEMU
    # argument so paths such as `file=C:\Program Files\...` stay a single
    # argument for QEMU's option parser.
    $headlessArgumentLine = (($arguments | ForEach-Object {
        '"' + $_.Replace('"', '\"') + '"'
    }) -join ' ')
    $process = Start-Process -FilePath $QemuPath -ArgumentList $headlessArgumentLine -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds($PreflightTimeoutSeconds)
    $menuObserved = $false
    while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
        if (Test-Path -LiteralPath $serialLog) {
            $currentLog = Get-Content -LiteralPath $serialLog -Raw
            if ((Get-MissingPreflightMarkers $currentLog $SyntheticInternalDiskCount $ExpectedImageName).Count -eq 0) {
                $menuObserved = $true
                Stop-Process -Id $process.Id -Force
                $process.WaitForExit()
                break
            }
        }
    }
    if (-not $menuObserved) {
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
            $process.WaitForExit()
        }
        if (Test-Path -LiteralPath $serialLog) {
            $currentLog = Get-Content -LiteralPath $serialLog -Raw
            $missing = Get-MissingPreflightMarkers $currentLog $SyntheticInternalDiskCount $ExpectedImageName
            throw "Headless QEMU preflight did not reach the expected NextBoot menu state within $PreflightTimeoutSeconds seconds. Missing log markers: $($missing -join '; ')"
        }
        throw "Headless QEMU preflight did not create a serial log within $PreflightTimeoutSeconds seconds."
    }
    $exitCode = 0
}
else {
    & $QemuPath @arguments
    $exitCode = $LASTEXITCODE
}

if (Test-Path -LiteralPath $serialLog) {
    Write-Host "Serial log: $serialLog"
    $logText = Get-Content -LiteralPath $serialLog -Raw
    Get-Content -LiteralPath $serialLog -Tail 120

    $missingMarkers = Get-MissingPreflightMarkers $logText $SyntheticInternalDiskCount $ExpectedImageName
    if ($missingMarkers.Count -gt 0) {
        throw "QEMU preflight did not reach the expected NextBoot menu state. Missing log markers: $($missingMarkers -join '; ')"
    }
    Write-Host 'QEMU preflight passed: UEFI boot, storage scan, and NextBoot menu were observed.'
    if ($ExpectedImageName) {
        Write-Host "QEMU preflight passed: expected image /ISO/$ExpectedImageName was listed."
    }
}
else {
    throw 'QEMU did not create a serial log; the preflight result is not valid.'
}
if ($exitCode -ne 0) {
    throw "QEMU exited with code $exitCode. Review the serial log above."
}
