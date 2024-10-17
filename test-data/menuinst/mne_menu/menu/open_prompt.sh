#!/bin/bash

# This is used to initialize the bash prompt on macOS and Linux.

if [[ -f ~/.bashrc ]] && [[ ${OSTYPE} != 'darwin'* ]]; then
    source ~/.bashrc
fi
source __PREFIX__/bin/activate
echo "Using $(python --version) from $(which python)"
echo "This is $(mne --version)"
