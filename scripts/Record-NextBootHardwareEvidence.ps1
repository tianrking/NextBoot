[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(0, 999)]
    [int] $DiskNumber,

    [Parameter(Mandatory = $true)]
    [ValidateSet('pass', 'fail', 'partial', 'blocked', 'unknown')]
    [string] $Result,

    [ValidateSet('auto', 'fixed', 'nvme', 'sata', 'usb', 'sd', 'enclosure', 'other')]
    [string] $Media = 'auto',

    [ValidateSet('auto', 'nvme', 'sata', 'ahci', 'usb', 'sd', 'virtio', 'other')]
    [string] $Bus = 'auto',

    [ValidateSet('auto', 'single', 'split')]
    [string] $Layout = 'auto',

    [string] $DataFilesystem = 'auto',

    [ValidateSet('iso', 'windows', 'wimboot', 'vlnk', 'img', 'vhd', 'vhdx', 'vdi', 'linux', 'plugins', 'mixed', 'unknown')]
    [string] $ImageType = 'iso',

    [string] $Firmware = 'unknown',

    [string] $Notes = '',

    [string] $ImagePath,

    [string[]] $EvidencePath = @(),

    [string] $OutputPath,

    [switch] $AppendCsv,

    [string] $CsvPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Normalize-MatrixValue([string] $Value) {
    return $Value.Trim().ToLowerInvariant().Replace('_', '-')
}

function Get-GitValue([string[]] $Arguments, [string] $Fallback) {
    try {
        $value = & git -C $projectRoot @Arguments 2>$null
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($value)) {
            return $value.Trim()
        }
    }
    catch {
        # The report remains useful outside a Git checkout.
    }
    return $Fallback
}

function Get-DefaultMedia([object] $Disk) {
    switch ($Disk.BusType.ToString().ToLowerInvariant()) {
        'nvme' { return 'nvme' }
        'sata' { return 'sata' }
        'usb' { return 'usb' }
        default { return 'other' }
    }
}

function Get-DefaultBus([object] $Disk) {
    switch ($Disk.BusType.ToString().ToLowerInvariant()) {
        'nvme' { return 'nvme' }
        'sata' { return 'sata' }
        'usb' { return 'usb' }
        default { return 'other' }
    }
}

function Escape-Markdown([string] $Value) {
    return $Value.Replace('|', '\\|').Replace("`r", ' ').Replace("`n", ' ')
}

$projectRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($CsvPath)) {
    $CsvPath = Join-Path $projectRoot 'docs\hardware\hardware-matrix.csv'
}

$disk = Get-Disk -Number $DiskNumber
$partitions = @(Get-Partition -DiskNumber $DiskNumber | Sort-Object PartitionNumber)
if ($partitions.Count -eq 0) {
    throw "Disk $DiskNumber has no partitions; do not record it as a NextBoot hardware result."
}

$partitionInventory = foreach ($partition in $partitions) {
    $volume = Get-Volume -Partition $partition -ErrorAction SilentlyContinue
    [pscustomobject]@{
        PartitionNumber = $partition.PartitionNumber
        GptType = $partition.GptType
        Type = $partition.Type
        DriveLetter = if ($null -eq $volume) { '' } else { $volume.DriveLetter }
        Label = if ($null -eq $volume) { '' } else { $volume.FileSystemLabel }
        FileSystem = if ($null -eq $volume) { '' } else { $volume.FileSystem }
        Size = $partition.Size
    }
}

$dataVolume = @($partitionInventory | Where-Object { -not [string]::IsNullOrWhiteSpace($_.FileSystem) } | Sort-Object Size -Descending | Select-Object -First 1)
if ($Layout -eq 'auto') {
    $Layout = if ($partitions.Count -ge 2) { 'split' } else { 'single' }
}
if ($DataFilesystem -eq 'auto') {
    $DataFilesystem = if ($dataVolume.Count -eq 1) { Normalize-MatrixValue $dataVolume[0].FileSystem } else { 'unknown' }
}
if ($Media -eq 'auto') { $Media = Get-DefaultMedia $disk }
if ($Bus -eq 'auto') { $Bus = Get-DefaultBus $disk }

$imageMetadata = $null
if (-not [string]::IsNullOrWhiteSpace($ImagePath)) {
    $resolvedImage = Resolve-Path -LiteralPath $ImagePath
    $imageItem = Get-Item -LiteralPath $resolvedImage
    $imageMetadata = [pscustomobject]@{
        Path = $imageItem.FullName
        Length = $imageItem.Length
        Sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $imageItem.FullName).Hash
    }
}

$resolvedEvidence = @(foreach ($path in $EvidencePath) {
    if (-not [string]::IsNullOrWhiteSpace($path)) {
        (Resolve-Path -LiteralPath $path).Path
    }
})

$timestamp = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
$stampForFile = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$commit = Get-GitValue @('rev-parse', '--short', 'HEAD') 'unknown'
$branch = Get-GitValue @('branch', '--show-current') 'unknown'
$hostInfo = Get-CimInstance Win32_ComputerSystem
$os = Get-CimInstance Win32_OperatingSystem
$hostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$device = "Disk ${DiskNumber}: $($disk.FriendlyName)"

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $projectRoot "target\hardware-reports\$stampForFile-disk$DiskNumber-$Media-$Bus-$Result.md"
}
$outputDirectory = Split-Path -Parent $OutputPath
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null

$report = [System.Collections.Generic.List[string]]::new()
$report.Add('# NextBoot Hardware Compatibility Report')
$report.Add('')
$report.Add('## Summary')
$report.Add('')
$report.Add('| Field | Value |')
$report.Add('| --- | --- |')
foreach ($entry in @(
    @('Timestamp UTC', $timestamp), @('Git commit', $commit), @('Git branch', $branch),
    @('Result', $Result), @('Device', $device), @('Media', $Media), @('Bus', $Bus),
    @('Layout', $Layout), @('Data filesystem', $DataFilesystem),
    @('Logical sector size', $disk.LogicalSectorSize), @('Image type', $ImageType),
    @('Firmware', $Firmware), @('Notes', $(if ([string]::IsNullOrWhiteSpace($Notes)) { 'none' } else { $Notes }))
)) {
    $report.Add("| $($entry[0]) | $(Escape-Markdown ([string]$entry[1])) |")
}
$report.Add('')
$report.Add('## Host')
$report.Add('')
$report.Add('```text')
$report.Add("Computer=$($hostInfo.Model)")
$report.Add("Manufacturer=$($hostInfo.Manufacturer)")
$report.Add("Windows=$($os.Caption) $($os.Version)")
$report.Add("Architecture=$hostArch")
$report.Add('```')
$report.Add('')
$report.Add('## Disk and Volume Inventory')
$report.Add('')
$report.Add('```text')
$report.Add(($disk | Format-List Number,FriendlyName,BusType,PartitionStyle,OperationalStatus,HealthStatus,LogicalSectorSize,PhysicalSectorSize,Size | Out-String).TrimEnd())
$report.Add(($partitionInventory | Format-Table -AutoSize | Out-String).TrimEnd())
$report.Add('```')

if ($null -ne $imageMetadata) {
    $report.Add('')
    $report.Add('## Source Image')
    $report.Add('')
    $report.Add('| Field | Value |')
    $report.Add('| --- | --- |')
    $report.Add("| Path | $(Escape-Markdown $imageMetadata.Path) |")
    $report.Add("| Bytes | $($imageMetadata.Length) |")
    $report.Add("| SHA-256 | $($imageMetadata.Sha256) |")
}

$report.Add('')
$report.Add('## Operator Checklist')
$report.Add('')
$report.Add('- [ ] Firmware listed the selected device as a UEFI boot target.')
$report.Add('- [ ] NextBoot displayed its version and listed the expected image.')
$report.Add('- [ ] The selected image reached its recorded boot marker or OS handoff.')
$report.Add('- [ ] A reboot was attempted where persistence matters.')

if ($resolvedEvidence.Count -gt 0) {
    $report.Add('')
    $report.Add('## Evidence Paths')
    $report.Add('')
    foreach ($path in $resolvedEvidence) { $report.Add("- ``$path``") }
}

[System.IO.File]::WriteAllLines($OutputPath, $report, [Text.UTF8Encoding]::new($false))

if ($AppendCsv) {
    $csvDirectory = Split-Path -Parent $CsvPath
    if (-not [string]::IsNullOrWhiteSpace($csvDirectory)) {
        New-Item -ItemType Directory -Force -Path $csvDirectory | Out-Null
    }
    $record = [pscustomobject][ordered]@{
        timestamp = $timestamp
        commit = $commit
        branch = $branch
        host_arch = $hostArch
        device = $device
        media = (Normalize-MatrixValue $Media)
        bus = (Normalize-MatrixValue $Bus)
        layout = (Normalize-MatrixValue $Layout)
        data_fs = (Normalize-MatrixValue $DataFilesystem)
        sector_size = [string]$disk.LogicalSectorSize
        image_type = (Normalize-MatrixValue $ImageType)
        firmware = $Firmware
        result = $Result
        report = $OutputPath
        notes = $Notes
    }
    if (-not (Test-Path -LiteralPath $CsvPath)) {
        $record | Export-Csv -LiteralPath $CsvPath -NoTypeInformation -Encoding utf8
    }
    else {
        $record | ConvertTo-Csv -NoTypeInformation | Select-Object -Skip 1 | Add-Content -LiteralPath $CsvPath -Encoding utf8
    }
}

Write-Output "Wrote $OutputPath"
if ($AppendCsv) { Write-Output "Updated $CsvPath" }
