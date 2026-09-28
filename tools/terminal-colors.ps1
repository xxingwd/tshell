# Run inside the terminal being compared. Change its theme/opacity while this
# stays on screen; this prints only SGR text and does not change terminal state.
$esc = [char]27
Write-Host 'Compare Blue (34 / 38;5;4), bright Blue and literal RGB.'
Write-Host 'The two Blue columns must match within each row. DIM is intentionally faint.'
$samples = @(
    @{ Label = 'shell Blue '; Code = '34' },
    @{ Label = 'ash Blue   '; Code = '38;5;4' },
    @{ Label = 'bright Blue'; Code = '94' },
    @{ Label = 'RGB #245cbe'; Code = '38;2;36;92;190' }
)
foreach ($style in @(
    @{ Label = 'Normal'; Code = '0' },
    @{ Label = 'Bold  '; Code = '0;1' },
    @{ Label = 'DIM   '; Code = '0;2' }
)) {
    $line = $style.Label + '  '
    foreach ($sample in $samples) {
        $line += "$esc[$($style.Code);$($sample.Code)m$($sample.Label) XXXXX$esc[0m  "
    }
    [Console]::WriteLine($line)
}
