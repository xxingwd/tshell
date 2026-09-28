$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
$os = Get-CimInstance Win32_OperatingSystem
$processors = @(Get-CimInstance Win32_Processor)
$model = ($processors | Select-Object -First 1).Name
$cores = ($processors | Measure-Object NumberOfLogicalProcessors -Sum).Sum
$tick = 0
$disks = @()
while ($true) {
    if ($collect_memory) { $os = Get-CimInstance Win32_OperatingSystem }
    'TSHELL_METRICS_BEGIN'
    'os Windows {0}' -f $os.Version
    'release {0}' -f $os.Caption
    'arch {0}' -f $os.OSArchitecture
    'model {0}' -f $model
    'cores {0}' -f $cores
    'uptime {0:F3}' -f ([DateTime]::Now - $os.LastBootUpTime).TotalSeconds
    'sample_time {0:F3}' -f ([Diagnostics.Stopwatch]::GetTimestamp() / [Diagnostics.Stopwatch]::Frequency)
    'processes {0}' -f $os.NumberOfProcesses
    if ($collect_memory) {
        'MemTotal: {0}' -f $os.TotalVisibleMemorySize
        'MemAvailable: {0}' -f $os.FreePhysicalMemory
    }
    if ($collect_cpu) {
        # The raw PercentProcessorTime counter is idle time; timestamps use 100 ns.
        Get-CimInstance Win32_PerfRawData_PerfOS_Processor -ErrorAction SilentlyContinue | ForEach-Object {
            $key = if ($_.Name -eq '_Total') { 'cpu' } else { 'cpu{0}' -f $_.Name }
            $idle = [decimal]$_.PercentProcessorTime
            $active = [decimal]$_.Timestamp_Sys100NS - $idle
            '{0} {1:F0} 0 0 {2:F0}' -f $key, $active, $idle
        }
    }
    if ($collect_network) {
        $routes = @(Get-NetRoute -ErrorAction SilentlyContinue | Where-Object { $_.DestinationPrefix -in @('0.0.0.0/0', '::/0') } | Sort-Object @{Expression={ $_.RouteMetric + $_.InterfaceMetric }})
        if ($routes.Count) { 'primary if{0}' -f $routes[0].InterfaceIndex }
        Get-NetAdapter -ErrorAction SilentlyContinue | Where-Object Status -eq 'Up' | ForEach-Object {
            $adapter = $_
            $stats = $adapter | Get-NetAdapterStatistics -ErrorAction SilentlyContinue
            if ($stats) {
                'net:if{0} {1} {2}' -f $adapter.ifIndex, $stats.ReceivedBytes, $stats.SentBytes
                'netname if{0} {1}' -f $adapter.ifIndex, $adapter.Name
            }
        }
    }
    if ($collect_io) {
        Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk -ErrorAction SilentlyContinue | Where-Object Name -ne '_Total' | ForEach-Object {
            'iototal {0}' -f ($_.Name -replace ' ', '_')
            'io:{0} {1} {2:F0} {3} {4:F0}' -f ($_.Name -replace ' ', '_'), $_.DiskReadsPersec, ([decimal]$_.DiskReadBytesPersec / 512), $_.DiskWritesPersec, ([decimal]$_.DiskWriteBytesPersec / 512)
        }
    }
    if ($collect_disk) {
        if ($tick -eq 0) {
            $disks = @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' -ErrorAction SilentlyContinue | Where-Object Size -gt 0 | ForEach-Object {
                $total = [math]::Floor($_.Size / 1024)
                $free = [math]::Floor($_.FreeSpace / 1024)
                $row = '{0} {1} {2} {3} {4:F0}% {0}' -f $_.DeviceID, $total, ($total - $free), $free, (100 * ($total - $free) / $total)
                'mount {0}' -f $row
                if ($_.DeviceID -eq $os.SystemDrive) { 'disk {0}' -f $row }
            }
            )
        }
        $disks
    }
    if ($collect_memory) {
        $pages = @(Get-CimInstance Win32_PageFileUsage -ErrorAction SilentlyContinue)
        if ($pages.Count) {
            'pagefile {0} {1}' -f (($pages | Measure-Object CurrentUsage -Sum).Sum * 1024), (($pages | Measure-Object AllocatedBaseSize -Sum).Sum * 1024)
        }
    }
    $tick = ($tick + 1) % 6
    'TSHELL_METRICS_END'
    Start-Sleep -Seconds 5
}
