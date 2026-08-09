<#
.SYNOPSIS
    winmux spike 앱의 private working set(WebView2 프로세스 트리 포함)을 주기적으로 측정한다.

.DESCRIPTION
    계획 v2 3장 "리소스 측정" / spike-plan.md 6장 체크리스트 6번의 실행 도구다.
    대상 프로세스(exe 이름)를 루트로 삼아, 부모 PID 체인을 따라가며 그 자손 프로세스
    (WebView2 렌더러/GPU 프로세스 등)를 전부 찾아낸 뒤, 각 프로세스의
    Private Working Set(Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate)을
    합산해 샘플마다 기록한다.

.PARAMETER ProcessName
    측정 대상 프로세스 이름(확장자 제외). 기본값 winmux-spike
    (apps/spike/src-tauri/tauri.conf.json의 productName과 일치).

.PARAMETER IntervalSec
    샘플링 간격(초). 기본값 5.

.PARAMETER Samples
    총 샘플 횟수. 기본값 12 (IntervalSec 5초 기준 총 약 60초).

.PARAMETER OutCsv
    결과를 저장할 CSV 파일 경로. 지정하면 프로세스별 내역 + 샘플별 TOTAL 행을 기록한다.
    지정하지 않으면 콘솔 표만 출력한다.

.EXAMPLE
    .\measure.ps1
    기본값(winmux-spike, 5초 간격, 12샘플)으로 콘솔에만 출력.

.EXAMPLE
    .\measure.ps1 -ProcessName winmux-spike -IntervalSec 5 -Samples 12 -OutCsv .\ram-4pane.csv
    spike-plan.md 6장 시나리오(터미널 4개 등)를 CSV로 기록.

.NOTES
    관리자 권한이 필요 없다 — Get-CimInstance로 Win32_Process(부모 PID 조회)와
    Win32_PerfFormattedData_PerfProc_Process(WorkingSetPrivate 조회)를 읽는 것,
    Get-Process로 현재 사용자 소유 프로세스 이름을 읽는 것 모두 일반 사용자 권한으로 가능하다.
#>

[CmdletBinding()]
param(
    [string]$ProcessName = "winmux-spike",
    [int]$IntervalSec = 5,
    [int]$Samples = 12,
    [string]$OutCsv
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-ProcessTreeIds {
    <#
        루트 프로세스 이름(exe)으로 시작해 Win32_Process 테이블을 한 번 읽고,
        부모 PID 체인으로 연결된 모든 자손 PID를 반복적으로 수집한다.
        WebView2는 별도 프로세스 트리(브라우저/렌더러/GPU)로 뜨므로 이렇게 해야
        전체 비용이 잡힌다.
    #>
    param([string]$RootName)

    $allProcs = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
    $rootIds = $allProcs |
        Where-Object { $_.Name -eq "$RootName.exe" } |
        Select-Object -ExpandProperty ProcessId

    $resultIds = [System.Collections.Generic.HashSet[uint32]]::new()
    foreach ($id in $rootIds) {
        [void]$resultIds.Add([uint32]$id)
    }

    if ($resultIds.Count -eq 0) {
        return @()
    }

    # 자손 탐색: 이번 회전에서 새로 추가된 PID가 없을 때까지 반복한다.
    # (트리 깊이가 얼마든 수렴한다 — WebView2 자손이 몇 단계든 상관없다.)
    $changed = $true
    while ($changed) {
        $changed = $false
        foreach ($p in $allProcs) {
            $ppid = [uint32]$p.ParentProcessId
            $pid_ = [uint32]$p.ProcessId
            if ($resultIds.Contains($ppid) -and -not $resultIds.Contains($pid_)) {
                [void]$resultIds.Add($pid_)
                $changed = $true
            }
        }
    }

    # HashSet을 그대로 return하면 파이프라인이 원소 단위로 unroll해 호출부 타입이
    # 흔들린다 (0개 → $null, 1개 → 스칼라). 항상 배열로 평탄화해 돌려준다 —
    # Windows PowerShell 5.1의 StrictMode에서는 $null/스칼라의 .Count 접근이 예외다.
    return @($resultIds)
}

function Get-SampleRows {
    param([string]$RootName, [datetime]$Timestamp)

    $ids = @(Get-ProcessTreeIds -RootName $RootName)
    if ($ids.Count -eq 0) {
        Write-Warning "프로세스 '$RootName'을 찾지 못했다 (아직 실행 전이거나 이름이 다를 수 있음)."
        return @()
    }

    # WorkingSetPrivate은 바이트 단위. 이 클래스의 Name은 "이름#N" 형태로 중복 인스턴스를
    # 구분하므로 이름 대신 IDProcess(실제 PID)로 매칭한다.
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
        Where-Object { $ids -contains [uint32]$_.IDProcess }

    $rows = foreach ($p in $perf) {
        $procInfo = Get-Process -Id $p.IDProcess -ErrorAction SilentlyContinue
        [PSCustomObject]@{
            Timestamp           = $Timestamp
            ProcessId           = [int]$p.IDProcess
            ProcessName         = if ($procInfo) { $procInfo.ProcessName } else { $p.Name }
            PrivateWorkingSetMB = [math]::Round($p.WorkingSetPrivate / 1MB, 2)
        }
    }

    return @($rows)
}

Write-Host "대상: $ProcessName (자손 프로세스 트리 포함, WebView2 포함)"
Write-Host "간격: ${IntervalSec}s / 샘플: $Samples"
Write-Host ""

$allRows = [System.Collections.Generic.List[object]]::new()

for ($i = 1; $i -le $Samples; $i++) {
    $ts = Get-Date
    # 함수 반환도 파이프라인 unroll 대상이므로 호출부에서 다시 배열로 감싼다 (PS 5.1 호환).
    $rows = @(Get-SampleRows -RootName $ProcessName -Timestamp $ts)

    if ($rows.Count -eq 0) {
        if ($i -lt $Samples) {
            Start-Sleep -Seconds $IntervalSec
        }
        continue
    }

    $totalMB = [math]::Round(($rows | Measure-Object -Property PrivateWorkingSetMB -Sum).Sum, 2)

    Write-Host ("[{0}] 샘플 {1}/{2}" -f $ts.ToString("HH:mm:ss"), $i, $Samples)
    $rows | Sort-Object -Property PrivateWorkingSetMB -Descending |
        Format-Table -Property ProcessId, ProcessName, PrivateWorkingSetMB -AutoSize | Out-Host
    Write-Host ("  합계: {0} MB ({1}개 프로세스)" -f $totalMB, $rows.Count)
    Write-Host ""

    foreach ($r in $rows) { $allRows.Add($r) }
    $allRows.Add([PSCustomObject]@{
        Timestamp           = $ts
        ProcessId           = $null
        ProcessName         = "TOTAL"
        PrivateWorkingSetMB = $totalMB
    })

    if ($i -lt $Samples) {
        Start-Sleep -Seconds $IntervalSec
    }
}

if ($OutCsv) {
    $allRows | Export-Csv -Path $OutCsv -NoTypeInformation -Encoding UTF8
    Write-Host "CSV 저장: $OutCsv"
}
