:: This is used to initialize the bash prompt on Windows.
@ECHO OFF

call %__PREFIX__%\Scripts\Activate.bat
FOR /F "tokens=*" %%g IN ('python --version') do (SET PYVER=%%g)
FOR /F "tokens=*" %%g IN ('where python') do (SET PYPATH=%%g)
FOR /F "tokens=*" %%g IN ('mne --version') do (SET MNEVER=%%g)

ECHO Using %PYVER% from %PYPATH%
ECHO This is %MNEVER%
