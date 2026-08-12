$ErrorActionPreference = 'Stop'

$version  = '1.6.15'
$url      = "https://github.com/oxide-sloc/oxide-sloc/releases/download/v$version/oxide-sloc-windows-x64.zip"
$checksum = 'f06067ed3bdedba6a5ae4a67b9642a8b5e688e7169cd6ab633ae4851b3ff2f8d'

$packageArgs = @{
    packageName    = $env:ChocolateyPackageName
    unzipLocation  = "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)"
    url            = $url
    checksum       = $checksum
    checksumType   = 'sha256'
}

Install-ChocolateyZipPackage @packageArgs

$binDir = Join-Path $packageArgs.unzipLocation 'oxide-sloc.exe'
Install-BinFile -Name 'oxide-sloc' -Path $binDir
