$ErrorActionPreference = 'Stop'
$engine = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$objdump = (Get-Command llvm-objdump -CommandType Application).Source
$strip = (Get-Command llvm-strip -CommandType Application).Source
$destination = Join-Path $PSScriptRoot 'redistributables'
$staging = Join-Path $PSScriptRoot ('.template-' + [Guid]::NewGuid())
$stagedDlls = Join-Path $staging 'redistributables'
New-Item -ItemType Directory -Path $stagedDlls | Out-Null

try {
    Push-Location -LiteralPath $engine
    try {
        $exe = $null
        cargo build --locked --release --no-default-features -p tellurium-runtime --bin tellurium_runtime --message-format=json-render-diagnostics |
            ForEach-Object {
                $message = ConvertFrom-Json -InputObject $_
                if ($message.executable -and $message.target.name -eq 'tellurium_runtime') {
                    $exe = $message.executable
                }
            }
        if ($LASTEXITCODE -ne 0 -or !$exe -or [IO.Path]::GetExtension($exe) -ne '.exe') {
            throw 'Cargo did not build a Windows runtime executable.'
        }
    } finally {
        Pop-Location
    }

    $stagedExe = Join-Path $staging 'tellurium_runtime.exe'
    Copy-Item -LiteralPath $exe -Destination $stagedExe
    & $strip --strip-all $stagedExe
    if ($LASTEXITCODE -ne 0) {
        throw 'Cannot strip symbols from the standalone runtime.'
    }

    $queue = [Collections.Generic.Queue[string]]::new()
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $queue.Enqueue($stagedExe)
    while ($queue.Count -gt 0) {
        $binary = $queue.Dequeue()
        $imports = & $objdump -p $binary | Select-String '^\s*DLL Name:\s+([^\s]+)'
        if ($LASTEXITCODE -ne 0 -or !$imports) {
            throw "Cannot inspect Windows imports of $binary"
        }
        foreach ($import in $imports) {
            $name = $import.Matches[0].Groups[1].Value
            if (!$seen.Add($name)) {
                continue
            }
            # DX12, D3DCompiler, XInput and the UCRT API sets ship with Windows 10/11.
            $compilerRuntime = $name -match '^(lib(c\+\+|unwind|stdc\+\+|gcc|winpthread)|vcruntime|msvcp|concrt)'
            if (!$compilerRuntime -and ($name -match '^(api|ext)-ms-win-' -or (Test-Path -LiteralPath (Join-Path $env:SystemRoot "System32/$name")))) {
                continue
            }
            $dll = (Get-Command $name -CommandType Application -ErrorAction Stop).Source
            Copy-Item -LiteralPath $dll -Destination $stagedDlls
            $queue.Enqueue($dll)
            if ($name -in 'libc++.dll', 'libunwind.dll') {
                $license = Join-Path (Split-Path (Split-Path $dll)) 'LICENSE.TXT'
                Copy-Item -LiteralPath $license -Destination (Join-Path $stagedDlls 'LLVM-MinGW-LICENSE.txt') -Force
            }
        }
    }

    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Copy-Item -LiteralPath $stagedExe -Destination $PSScriptRoot -Force
    Get-ChildItem -LiteralPath $destination -File | ForEach-Object {
        if (!(Test-Path -LiteralPath (Join-Path $stagedDlls $_.Name))) {
            Remove-Item -LiteralPath $_.FullName
        }
    }
    Get-ChildItem -LiteralPath $stagedDlls -File | Copy-Item -Destination $destination -Force
} finally {
    Get-ChildItem -LiteralPath $stagedDlls -File | Remove-Item
    Remove-Item -LiteralPath $stagedDlls
    Get-ChildItem -LiteralPath $staging -File | Remove-Item
    Remove-Item -LiteralPath $staging
}
