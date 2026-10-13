# Use the full installer to provision the control device's user ACL.
param([string]$PackageDir)
throw 'Direct AAP installation is no longer supported. Build a complete NTPods distribution and use NTPods.msi or its generated install.ps1; those set the intended user SID before either driver starts.'
