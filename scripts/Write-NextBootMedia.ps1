[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $ImagePath,

    [ValidateRange(-1, 999)]
    [int] $DiskNumber = -1,

    [string] $TargetPath,

    [switch] $VerifyOnly,

    [switch] $ConfirmErase
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run PowerShell as Administrator, then run this command again.'
    }
}

function Get-PhysicalDrivePath([int] $Number) {
    return "\\.\PhysicalDrive$Number"
}

function Read-Exact([System.IO.FileStream] $Stream, [byte[]] $Buffer, [int] $Count) {
    $offset = 0
    while ($offset -lt $Count) {
        $read = $Stream.Read($Buffer, $offset, $Count - $offset)
        if ($read -eq 0) { throw 'Unexpected end of device while reading.' }
        $offset += $read
    }
}

function Write-Exact([System.IO.FileStream] $Stream, [byte[]] $Buffer, [int] $Count) {
    $Stream.Write($Buffer, 0, $Count)
}

function Test-ByteRange([System.IO.FileStream] $Source, [System.IO.FileStream] $Target, [Int64] $Length) {
    function Get-RangeDigest([System.IO.FileStream] $Stream, [Int64] $RangeLength) {
        $chunkSize = 4MB
        $buffer = [byte[]]::new($chunkSize)
        $hash = [Security.Cryptography.IncrementalHash]::CreateHash('SHA256')
        try {
            $Stream.Position = 0
            $processed = [Int64]0
            while ($processed -lt $RangeLength) {
                $count = [int][Math]::Min([Int64]$chunkSize, $RangeLength - $processed)
                Read-Exact $Stream $buffer $count
                $hash.AppendData($buffer, 0, $count)
                $processed += $count
                Write-Progress -Activity 'Verifying NextBoot media' -Status "$([Math]::Floor($processed / 1MB)) MiB / $([Math]::Floor($RangeLength / 1MB)) MiB" -PercentComplete ([int](50 * $processed / $RangeLength))
            }
            return ([BitConverter]::ToString($hash.GetHashAndReset())).Replace('-', '')
        }
        finally {
            $hash.Dispose()
        }
    }

    $sourceDigest = Get-RangeDigest $Source $Length
    $targetDigest = Get-RangeDigest $Target $Length
    Write-Progress -Activity 'Verifying NextBoot media' -Completed
    if ($sourceDigest -ne $targetDigest) {
        throw "Verification failed: full-range SHA-256 differs ($sourceDigest != $targetDigest)."
    }
}

function Get-NextBootDataStart([System.IO.FileStream] $Source) {
    $sector = [byte[]]::new(512)
    $Source.Position = 512
    Read-Exact $Source $sector 512
    if ([Text.Encoding]::ASCII.GetString($sector, 0, 8) -ne 'EFI PART') {
        throw 'The selected image does not have a GPT header at LBA 1.'
    }
    $entriesLba = [BitConverter]::ToUInt64($sector, 72)
    $entryCount = [BitConverter]::ToUInt32($sector, 80)
    $entrySize = [BitConverter]::ToUInt32($sector, 84)
    if ($entrySize -lt 128 -or $entrySize -gt 4096) { throw 'The GPT partition-entry size is invalid.' }
    $entry = [byte[]]::new($entrySize)
    for ($index = 0; $index -lt $entryCount; $index++) {
        $Source.Position = [Int64]$entriesLba * 512 + [Int64]$index * $entrySize
        Read-Exact $Source $entry $entrySize
        $name = [Text.Encoding]::Unicode.GetString($entry, 56, 72).TrimEnd([char]0)
        if ($name -eq 'NEXBOOT_DATA') {
            return [BitConverter]::ToUInt64($entry, 32)
        }
    }
    throw 'The selected image has no GPT partition named NEXBOOT_DATA.'
}

function Test-NextBootBootRecords([System.IO.FileStream] $Source, [System.IO.FileStream] $Target) {
    $dataStart = Get-NextBootDataStart $Source
    $expected = [byte[]]::new(512)
    $actual = [byte[]]::new(512)
    $Source.Position = [Int64]$dataStart * 512
    $Target.Position = [Int64]$dataStart * 512
    Read-Exact $Source $expected 512
    Read-Exact $Target $actual 512
    if ([Text.Encoding]::ASCII.GetString($actual, 3, 8) -ne 'EXFAT   ' -or $actual[510] -ne 0x55 -or $actual[511] -ne 0xAA) {
        throw 'NEXTDATA does not contain a valid exFAT boot record after writing.'
    }
    if (-not [Linq.Enumerable]::SequenceEqual([byte[]]$expected, [byte[]]$actual)) {
        throw 'NEXTDATA exFAT boot record differs from the selected image.'
    }
}

$image = Get-Item -LiteralPath $ImagePath -ErrorAction Stop
if ($image.PSIsContainer) { throw 'ImagePath must name an extracted raw .img file, not a directory.' }
$imageLength = [Int64]$image.Length
if ($imageLength -lt 34MB) { throw 'ImagePath is too small to be a NextBoot raw image.' }

$source = [System.IO.File]::Open($image.FullName, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
$target = $null
$wasOffline = $true
try {
    if ($TargetPath) {
        if ($DiskNumber -ge 0) { throw 'Specify either TargetPath or DiskNumber, not both.' }
        $targetItem = Get-Item -LiteralPath $TargetPath -ErrorAction Stop
        if ($targetItem.PSIsContainer) { throw 'TargetPath must name a raw-image file, not a directory.' }
        if ([Int64]$targetItem.Length -lt $imageLength) { throw 'TargetPath is smaller than ImagePath.' }
        $target = [System.IO.File]::Open($targetItem.FullName, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
        Test-NextBootBootRecords $source $target
        Test-ByteRange $source $target $imageLength
        Write-Output "Verified raw target $($targetItem.Name): the full range matches $($image.Name)."
        return
    }

    if ($DiskNumber -lt 0) { throw 'Specify DiskNumber for a physical write or TargetPath for verification.' }
    Require-Administrator
    $disk = Get-Disk -Number $DiskNumber -ErrorAction Stop
    if ($disk.IsBoot -or $disk.IsSystem) { throw "Refusing to access system or boot disk $DiskNumber." }
    if ([Int64]$disk.Size -lt $imageLength) { throw "Disk $DiskNumber is smaller than the image ($($disk.Size) bytes < $imageLength bytes)." }
    $physicalPath = Get-PhysicalDrivePath $DiskNumber
    $wasOffline = [bool]$disk.IsOffline
    if ($VerifyOnly) {
        $target = [System.IO.File]::Open($physicalPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        Test-NextBootBootRecords $source $target
        Test-ByteRange $source $target $imageLength
        Write-Output "Verified Disk ${DiskNumber}: the full written range matches $($image.Name), including the NEXTDATA exFAT boot record."
        return
    }

    if (-not $ConfirmErase) {
        $answer = Read-Host "Disk ${DiskNumber} ($($disk.FriendlyName), $([Math]::Floor($disk.Size / 1GB)) GB) will be erased. Type the disk number to continue"
        if ($answer -ne [string]$DiskNumber) { throw 'Cancelled. No data was written.' }
    }

    if (-not $wasOffline) { Set-Disk -Number $DiskNumber -IsOffline $true }
    $target = [System.IO.File]::Open($physicalPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite)
    $buffer = [byte[]]::new(4MB)
    $written = [Int64]0
    while ($written -lt $imageLength) {
        $count = [int][Math]::Min([Int64]$buffer.Length, $imageLength - $written)
        Read-Exact $source $buffer $count
        Write-Exact $target $buffer $count
        $written += $count
        Write-Progress -Activity 'Writing NextBoot media' -Status "$([Math]::Floor($written / 1MB)) MiB / $([Math]::Floor($imageLength / 1MB)) MiB" -PercentComplete ([int](100 * $written / $imageLength))
    }
    $target.Flush($true)
    Write-Progress -Activity 'Writing NextBoot media' -Completed
    Test-NextBootBootRecords $source $target
    Test-ByteRange $source $target $imageLength
    Write-Output "Wrote and verified Disk $DiskNumber. Safely remove it, then open NEXTDATA and copy boot images into ISO."
}
finally {
    if ($null -ne $target) { $target.Dispose() }
    $source.Dispose()
    if (-not $wasOffline) {
        Set-Disk -Number $DiskNumber -IsOffline $false -ErrorAction SilentlyContinue
        Update-HostStorageCache
    }
}
