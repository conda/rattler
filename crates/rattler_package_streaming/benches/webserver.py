from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
import requests
import io

# Path to the Conda files
PYTHON_FILE_URL = "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2"
BOLTONS_FILE_URL = "https://repo.prefix.dev/conda-forge/noarch/boltons-21.0.0-pyhd8ed1ab_0.tar.bz2"
BAT_FILE_URL = "https://repo.prefix.dev/conda-forge/win-64/bat-0.22.1-h7f3b576_0.tar.bz2"

app = FastAPI()

# In-memory storage for files
file_cache = {}

async def fetch_conda_file(url: str) -> io.BytesIO:
    """Fetch the Conda file and return it as an in-memory BytesIO object."""
    response = requests.get(url, stream=True)
    response.raise_for_status()

    memory_file = io.BytesIO()
    for chunk in response.iter_content(chunk_size=8192):
        memory_file.write(chunk)
    memory_file.seek(0)
    return memory_file

@app.on_event("startup")
async def load_files():
    """Download Conda files at server startup."""
    global file_cache
    print("Downloading Conda files...")
    file_cache["python"] = await fetch_conda_file(PYTHON_FILE_URL)
    file_cache["boltons"] = await fetch_conda_file(BOLTONS_FILE_URL)
    file_cache["bat"] = await fetch_conda_file(BAT_FILE_URL)
    print("Conda files downloaded and stored in memory.")


@app.get("/{file_name}")
async def serve_file(file_name: str):
    """Serve the requested file from in-memory storage."""
    if file_name not in file_cache:
        raise HTTPException(status_code=404, detail="File not found")

    # Retrieve the file and reset the pointer
    memory_file = file_cache[file_name]
    memory_file.seek(0)  # Reset the pointer to the start of the file
    
    return StreamingResponse(
        memory_file,
        media_type="application/octet-stream",
        headers={"Content-Disposition": f"attachment; filename={file_name}.tar.bz2"},
    )


@app.get("/")
def list_files():
    """List available files."""
    return {"available_files": list(file_cache.keys())}
