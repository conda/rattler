use crate::patching::OpenFile;
use crate::tree::EnvTree;
use crate::tree_objects::Node;
use fuser::consts::FOPEN_KEEP_CACHE;
use fuser::{
    FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyOpen, Request,
};
use libc::ENOENT;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;
use std::vec;

// All data is read-only so it can be cached forever
const TTL: Duration = Duration::from_secs(365 * 24 * 60 * 60);

pub struct RattlerFS {
    tree: EnvTree,
    uid: u32,
    gid: u32,
    open_files: HashMap<u64, OpenFile>,
}

impl RattlerFS {
    pub fn new(tree: EnvTree, uid: u32, gid: u32) -> RattlerFS {
        RattlerFS {
            tree,
            uid,
            gid,
            open_files: HashMap::new(),
        }
    }
}

impl Filesystem for RattlerFS {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if let Some(node) = self.tree.find_by_inode(parent) {
            if let Ok(dir) = node.borrow().as_directory() {
                if let Some(child) = dir.get_child(name) {
                    return reply.entry(&TTL, &child.borrow().stat(self.uid, self.gid), 0);
                }
            }
        }
        reply.error(ENOENT);
    }

    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        if let Some(node) = self.tree.find_by_inode(ino) {
            if let Ok(symlink) = node.borrow().as_symlink() {
                return reply.data(symlink.readlink().as_os_str().as_bytes());
            }
        }
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        if let Some(node) = self.tree.find_by_inode(ino) {
            return reply.attr(&TTL, &node.borrow().stat(self.uid, self.gid));
        }
        reply.error(ENOENT);
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let node = self.tree.find_by_inode(ino);
        match node {
            Some(node) => {
                let borrowed = node.borrow();
                let open_file = borrowed.open();
                let fd = open_file.fd();
                self.open_files.insert(fd, open_file);
                reply.opened(fd, FOPEN_KEEP_CACHE);
            }
            None => {
                reply.error(ENOENT);
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        if self.open_files.remove(&fh).is_none() {
            reply.error(ENOENT);
        } else {
            reply.ok();
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let mut buffer = vec![0; size as usize];
        let f = self.open_files.get_mut(&fh).unwrap();
        if let Ok(bytes_read) = f.read_at(&mut buffer, offset as u64) {
            reply.data(&buffer[..bytes_read]);
        } else {
            panic!("Failed to read from file");
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let node = self.tree.find_by_inode(ino);
        match node {
            Some(node) => {
                let borrowed = node.borrow();
                match &*borrowed {
                    Node::Directory(dir) => {
                        let entries = dir.children();
                        if offset <= 0 {
                            let _ = reply.add(ino, 1, FileType::Directory, ".");
                        }
                        if offset <= 1 {
                            let _ = if ino == self.tree.root_ino() {
                                reply.add(ino, 2, FileType::Directory, "..")
                            } else {
                                match &*borrowed.parent().borrow() {
                                    Node::Directory(_) => reply.add(
                                        borrowed.parent().borrow().ino(),
                                        2,
                                        FileType::Directory,
                                        "..",
                                    ),
                                    _ => {
                                        unreachable!()
                                    }
                                }
                            };
                        }
                        for (i, entry) in
                            entries.into_iter().enumerate().skip((offset - 2) as usize)
                        {
                            let borrowed = entry.borrow();
                            let kind: FileType = match &*borrowed {
                                Node::Directory(_) => FileType::Directory,
                                Node::File(_) | Node::EntryPoint(_) => FileType::RegularFile,
                                Node::Symlink(_) => FileType::Symlink,
                            };
                            if reply.add(borrowed.ino(), (i + 3) as i64, kind, borrowed.name()) {
                                break;
                            }
                        }
                        reply.ok();
                    }
                    Node::Symlink(_) => {
                        panic!("TODO Symlink!");
                    }
                    Node::File(_) | Node::EntryPoint(_) => {
                        reply.error(ENOENT);
                    }
                }
            }
            None => {
                reply.error(ENOENT);
            }
        }
    }
}
