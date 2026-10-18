# Nạp môi trường build:  . .\env.ps1
$root = "B:\Bussiness\dsp\.toolchain"
$env:RUSTUP_HOME = "$root\rustup"
$env:CARGO_HOME  = "$root\cargo"
$env:PATH        = "$root\cargo\bin;$root\mingw64\bin;$env:PATH"
Write-Host "Toolchain da nap. rustc:" (& rustc -V)
