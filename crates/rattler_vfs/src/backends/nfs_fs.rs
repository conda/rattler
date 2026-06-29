use std::{ffi::OsStr, os::unix::ffi::OsStrExt, sync::Arc};

use nfs3_server::vfs::{
    FileHandleU64, NextResult, NfsFileSystem, NfsReadFileSystem, ReadDirIterator,
    ReadDirPlusIterator,
};
use nfs3_types::nfs3::{
    Nfs3Option, entryplus3, fattr3, filename3, ftype3, nfspath3, nfsstat3, nfstime3, post_op_attr,
    sattr3,
};

use crate::virtual_fs_core::{DirectoryEntry, VirtualAttr, VirtualFSCore};

pub struct NfsFS {
    pub inner: Arc<VirtualFSCore>,
}

fn to_fattr(attr: VirtualAttr, ino: u64) -> fattr3 {
    fattr3 {
        type_: if attr.is_dir {
            ftype3::NF3DIR
        } else if attr.is_symlink {
            ftype3::NF3LNK
        } else {
            ftype3::NF3REG
        },
        mode: attr.perm as u32,
        nlink: 1,
        uid: attr.uid,
        gid: attr.gid,
        size: attr.size,
        used: attr.size,
        rdev: Default::default(),
        fsid: 1,
        fileid: ino,
        atime: nfstime3::default(),
        mtime: nfstime3::default(),
        ctime: nfstime3::default(),
    }
}

fn nfs_error(e: anyhow::Error) -> nfsstat3 {
    eprintln!("NFS error: {e:?}");
    nfsstat3::NFS3ERR_IO
}

fn handle_to_ino(handle: &FileHandleU64) -> Result<usize, nfsstat3> {
    let raw = handle.as_u64();
    if raw == 0 {
        return Err(nfsstat3::NFS3ERR_BADHANDLE);
    }
    Ok(raw as usize)
}

impl NfsReadFileSystem for NfsFS {
    type Handle = FileHandleU64;

    fn root_dir(&self) -> FileHandleU64 {
        FileHandleU64::new(1)
    }

    async fn lookup(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
    ) -> Result<FileHandleU64, nfsstat3> {
        let parent_ino = handle_to_ino(dirid)?;
        let name = OsStr::from_bytes(filename.as_ref());
        let child_idx = self
            .inner
            .lookup(parent_ino, name)
            .ok_or(nfsstat3::NFS3ERR_NOENT)?;

        Ok(FileHandleU64::new((child_idx + 1) as u64))
    }

    async fn getattr(&self, id: &FileHandleU64) -> Result<fattr3, nfsstat3> {
        let ino = handle_to_ino(id)?;
        let attr = self.inner.getattr(ino).map_err(nfs_error)?;
        Ok(to_fattr(attr, id.as_u64()))
    }

    async fn read(
        &self,
        id: &FileHandleU64,
        offset: u64,
        count: u32,
    ) -> Result<(Vec<u8>, bool), nfsstat3> {
        let ino = handle_to_ino(id)?;
        let fh = self.inner.open_cached(ino).map_err(nfs_error)?;
        let data = self
            .inner
            .read(ino, fh, offset as usize, count as usize)
            .map_err(nfs_error)?;
        let eof = data.len() < count as usize;

        Ok((data, eof))
    }

    async fn readdir(
        &self,
        dirid: &FileHandleU64,
        cookie: u64,
    ) -> Result<impl ReadDirIterator, nfsstat3> {
        self.readdirplus(dirid, cookie).await
    }

    async fn readdirplus(
        &self,
        dirid: &FileHandleU64,
        cookie: u64,
    ) -> Result<impl ReadDirPlusIterator, nfsstat3> {
        let ino = handle_to_ino(dirid)?;
        let entries = self.inner.readdir(ino).map_err(nfs_error)?;

        Ok(NfsDirectoryIterator::new(
            self.inner.clone(),
            entries,
            cookie as usize,
        ))
    }

    async fn readlink(&self, id: &FileHandleU64) -> Result<nfspath3<'_>, nfsstat3> {
        let ino = handle_to_ino(id)?;
        let target: Vec<u8> = self.inner.readlink(ino).map_err(nfs_error)?;
        Ok(nfspath3::from(target))
    }
}

// Implement the full NfsFileSystem trait so that the ACCESS handler reports
// VFSCapabilities::ReadWrite (the default). With bind_ro / ReadOnly capabilities
// the handler strips ACCESS3_EXECUTE, preventing execve() from the NFS mount.
// All mutating operations return ROFS — the filesystem is effectively read-only.
impl NfsFileSystem for NfsFS {
    async fn setattr(&self, _id: &FileHandleU64, _setattr: sattr3) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn write(
        &self,
        _id: &FileHandleU64,
        _offset: u64,
        _data: &[u8],
    ) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn create(
        &self,
        _dirid: &FileHandleU64,
        _filename: &filename3<'_>,
        _attr: sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn create_exclusive(
        &self,
        _dirid: &FileHandleU64,
        _filename: &filename3<'_>,
    ) -> Result<FileHandleU64, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn mkdir(
        &self,
        _dirid: &FileHandleU64,
        _dirname: &filename3<'_>,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn remove(
        &self,
        _dirid: &FileHandleU64,
        _filename: &filename3<'_>,
    ) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn rename<'a>(
        &self,
        _from_dirid: &FileHandleU64,
        _from_filename: &filename3<'a>,
        _to_dirid: &FileHandleU64,
        _to_filename: &filename3<'a>,
    ) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn symlink<'a>(
        &self,
        _dirid: &FileHandleU64,
        _linkname: &filename3<'a>,
        _symlink: &nfspath3<'a>,
        _attr: &sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }
}

struct NfsDirectoryIterator {
    inner: Arc<VirtualFSCore>,
    entries: Vec<DirectoryEntry>,
    index: usize,
}

impl NfsDirectoryIterator {
    fn new(inner: Arc<VirtualFSCore>, entries: Vec<DirectoryEntry>, start: usize) -> Self {
        Self {
            inner,
            entries,
            index: start,
        }
    }
}

impl ReadDirPlusIterator for NfsDirectoryIterator {
    async fn next(&mut self) -> NextResult<entryplus3<'static>> {
        let Some(entry) = self.entries.get(self.index) else {
            return NextResult::Eof;
        };
        self.index += 1;

        // entry.ino is already a 1-based inode — pass directly to getattr
        let attr = self
            .inner
            .getattr(entry.ino as usize)
            .ok()
            .map(|attr| to_fattr(attr, entry.ino));

        NextResult::Ok(entryplus3 {
            fileid: entry.ino,
            name: filename3::from(entry.name.as_bytes().to_vec()),
            cookie: self.index as u64,
            name_attributes: attr.map_or(post_op_attr::None, post_op_attr::Some),
            name_handle: Nfs3Option::None,
        })
    }
}
