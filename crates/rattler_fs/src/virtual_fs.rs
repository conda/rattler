use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, INodeNo, LockOwner, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, Request
};
use libc::{EIO, ENOENT, ENOTDIR, getuid, getgid};
use rattler_conda_types::package::FileMode;
use std::{cmp::max, collections::HashMap, ffi::OsStr, fs::{self, File}, mem::take, os::unix::fs::{MetadataExt, PermissionsExt}, path::{Path, PathBuf}, sync::{Mutex, atomic::AtomicU64}, time::{Duration, SystemTime, UNIX_EPOCH}};


use memmap2::Mmap;

use crate::{fuse_directory::{FuseFile, FuseMetadata}, prefix_replacement::{binary_prefix_replacement,  text_prefix_replacement}};

const TTL: Duration = Duration::from_secs(5);

fn i64_to_systemtime(time: i64, nanos: i64 ) -> SystemTime {
    UNIX_EPOCH + Duration::new(time as u64, nanos as u32)
}
pub struct VirtualFS{
    metadata: Vec<FuseMetadata>,
    mount_point: PathBuf,
    open_files: Mutex<HashMap<u64, Mmap>>, 
    next_fh: AtomicU64
}

impl VirtualFS{
    
    pub fn new(metadata: &mut Vec<FuseMetadata>, mount_point: &Path) -> Self{
        VirtualFS{
            metadata: take(metadata),
            mount_point: mount_point.to_path_buf(),
            open_files: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
        }
    }

    fn _getpath(&self, file: &FuseFile) -> PathBuf {
        let mut path = (*file.cache_base_path).to_path_buf();
        let parent = self.metadata[file.parent].as_directory().unwrap();
        path = path.join(&parent.prefix_path); 
        path.join(&file.file_name)
    }

    fn _getattr(&self, child: &FuseMetadata, child_index: &usize) -> FileAttr{
        match child {
            FuseMetadata::Directory(_) => {
                FileAttr {
                    ino: INodeNo((child_index + 1) as u64),
                    size: 0,
                    blocks: 0,
                    atime: UNIX_EPOCH, // 1970-01-01 00:00:00
                    mtime: UNIX_EPOCH,
                    ctime: UNIX_EPOCH,
                    crtime: UNIX_EPOCH,
                    kind: FileType::Directory,
                    perm: 0o755,
                    nlink: 1,
                    uid: unsafe { getuid() },
                    gid: unsafe { getgid() },
                    rdev: 0,
                    flags: 0,
                    blksize: 512,
                }
            },
            FuseMetadata::File(file) => {
                let path = self._getpath(file);

                let metadata = fs::metadata(path).unwrap(); // TODO need to handle error
                
                FileAttr {
                    ino: INodeNo((child_index + 1) as u64),
                    size: metadata.len(),
                    blocks: metadata.blocks(),
                    atime: metadata.accessed().unwrap_or(UNIX_EPOCH), // default: 1970-01-01 00:00:00
                    mtime: metadata.modified().unwrap_or(UNIX_EPOCH),
                    ctime: i64_to_systemtime(metadata.ctime(), metadata.ctime_nsec()), 
                    crtime: metadata.created().unwrap_or(UNIX_EPOCH),
                    kind: FileType::RegularFile,
                    perm: (metadata.permissions().mode() & 0o777) as u16,
                    nlink: 1,
                    uid: unsafe {getuid()} ,
                    gid: unsafe {getgid()},
                    rdev: 0,
                    flags: 0,
                    blksize: 512,
                }
            }
        }
    }
}

impl Filesystem for VirtualFS {
    fn lookup(&self, 
        _req: &Request, 
        parent: INodeNo, 
        name: &OsStr, 
        reply: ReplyEntry
    ) {
        if parent > fuser::INodeNo(self.metadata.len() as u64) {
            reply.error(Errno::from_i32(ENOENT));
            return
        }

        let Some(parent_directory) = self.metadata[(parent-1) as usize].as_directory() else {
            reply.error(Errno::from_i32(ENOTDIR));
            return
        };


        for child_index in parent_directory.children.iter() {
            let child = &self.metadata[*child_index];

            if child.file_name() != name {
               continue
            }

            let attr = self._getattr(child, child_index);

            reply.entry(&TTL, &attr, fuser::Generation(0));
            
            return
        } 

        reply.error(Errno::from_i32(ENOENT));
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        if ino > fuser::INodeNo(self.metadata.len() as u64) {
            reply.error(Errno::from_i32(ENOENT));
            return
        }

        let index = (ino -1) as usize;

        let entry = &self.metadata[index];
        let attr = self._getattr(&entry, &index);

        reply.attr(&TTL, &attr);
    }

    //TODO 
    fn open(
        &self, 
        _req: &Request, 
        ino: INodeNo,
        _flags: OpenFlags, 
        reply: ReplyOpen
    ) {
        println!("open was called with ino {ino}");  // open from the cache, keep track of fs object 

        if ino > fuser::INodeNo(self.metadata.len() as u64) {
            reply.error(Errno::from_i32(ENOENT));
            return
        }

        let index = (ino -1) as usize;

        let Some(current_file) = self.metadata[index].as_file() else {
            reply.opened(fuser::FileHandle(0), FopenFlags::empty());
            return
        };

        let path = self._getpath(current_file);

        let Ok(file) = File::open(&path) else {
            println!("file didn't open {:#?}", &path);
            reply.error(Errno::from_i32(EIO));
            return
        };

        // don't the contents just go out of scope & dropped, What is the usefullness of these lines? - i assumed so commented out, just have to recheck?
        // let mut contents: Vec<u8> = Vec::new();
        // file.read_to_end(&mut contents);

        let mmap = unsafe { Mmap::map(&file).expect(&format!("failed to memmory map {path:#?}")) };

        let fh = self.next_fh.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let current_file = self.metadata[index].as_file_mut().expect("already used as a file");

        if let Some(prefix_placeholder) = &mut current_file.prefix_placeholder {
            prefix_placeholder.fill_offsets(&mmap);
        };

        self.open_files.lock().unwrap().insert(fh, mmap);

        reply.opened(FileHandle(fh), FopenFlags::empty());
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ){
        println!("closed inode {} at fh{}", ino, fh);
        if fh.eq(&FileHandle(0)) {
            reply.ok();
            return 
        }

        let Some(_) = self.open_files.lock().unwrap().remove(&fh) else {
            println!("releasing a non existing file");
            reply.error(Errno::from_i32(EIO));
            return
        };
        
        reply.ok();
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let index = (ino. -1) as usize;

        let Some(current_file) = self.metadata[index].as_file() else {
            reply.error(Errno::from_i32(EIO));
            return
        };

        println!("reading from {}, with offset {} and size {}", fh, offset, size);
        let lock = self.open_files.lock().unwrap();
        let Some(file) = lock.get(&fh) else{
            println!("error in file {}", &fh);
            reply.error(Errno::from_i32(EIO));
            return
        };

        let start = offset as usize;
        let end: usize = start + size as usize;
        
        match &current_file.prefix_placeholder {
            Some(placeholder) => {
                // let mut buffer = vec![0 as u8; size as usize];
                let buffer: Vec<u8>;
                match placeholder.file_mode{
                    FileMode::Text => {
                        buffer = text_prefix_replacement(placeholder, start, end, size as usize, file, &self.mount_point);
                    },
                    FileMode::Binary => {
                        buffer = binary_prefix_replacement(placeholder, start, end, size as usize, file, &self.mount_point);
                    },
                }
                reply.data(&buffer);
            },
            None =>  {
                let buffer = Vec::from_iter(file[start..end].iter().copied());
            
                reply.data(&buffer);
            }
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        if ino > fuser::INodeNo(self.metadata.len() as u64) {
            reply.error(Errno::from_i32(NonZeroI32::new(ENOENT).unwrap().into()));
            return
        }

        let Some(current_directory) = self.metadata[(ino-1) as usize].as_directory() else {
            reply.error(Errno::from_i32(NonZeroI32::new(ENOTDIR).unwrap().into()));
            return
        };

    
        if offset == 0 {
            if reply.add(INodeNo((current_directory.parent + 1) as u64), 1, FileType::Directory, "..") {
                reply.ok();
                return
            }
        }
        if offset <= 1 {
            if reply.add(INodeNo(ino.into()), 2, FileType::Directory, ".") {
                reply.ok();
                return
            }
        }

        for (i, child_index) in current_directory.children.iter().enumerate().skip(max(offset - 2, 0)  as usize) {
            // i + 1 means the index of the next entry
            // if the entry is added, then break the loop
            let child = &self.metadata[*child_index];

            let kind = match child {
                FuseMetadata::Directory(_) => { FileType::Directory },
                FuseMetadata::File(_) => { FileType::RegularFile }
            };

            if reply.add(INodeNo((child_index + 1) as u64), (i + 3) as u64, kind, child.file_name()) {

                break
            }

        }
        reply.ok();
    }
}
