// Copyright (c) 2004-present, Facebook, Inc.
// All Rights Reserved.
//
// This software may be used and distributed according to the terms of the
// GNU General Public License version 2 or any later version.

use std::marker::PhantomData;
use std::sync::Arc;

use futures::future::{BoxFuture, Future};
use futures::stream::{BoxStream, Stream};

use bookmarks::{self, Bookmarks, Version};
use changeset::Changeset;
use manifest::{BoxManifest, Manifest};
use nodehash::NodeHash;

pub type BoxedBookmarks<E> = Box<
    Bookmarks<
        Error = E,
        Value = NodeHash,
        Get = BoxFuture<Option<(NodeHash, Version)>, E>,
        Keys = BoxStream<Vec<u8>, E>,
    >,
>;

pub trait Repo: 'static {
    type Error: Send + 'static;

    /// Return a stream of all changeset ids
    ///
    /// This returns a Stream which produces each changeset that's reachable from a
    /// head exactly once. This does not guarantee any particular order, but something
    /// approximating a BFS traversal from the heads would be ideal.
    ///
    /// XXX Is "exactly once" too strong? This probably requires a "has seen" structure which
    /// will be O(changesets) in size. Probably OK up to 10-100M changesets.
    fn get_changesets(&self) -> BoxStream<NodeHash, Self::Error>;

    fn get_heads(&self) -> BoxStream<NodeHash, Self::Error>;
    fn get_bookmarks(&self) -> Result<BoxedBookmarks<Self::Error>, Self::Error>;
    fn changeset_exists(&self, nodeid: &NodeHash) -> BoxFuture<bool, Self::Error>;
    fn get_changeset_by_nodeid(&self, nodeid: &NodeHash) -> BoxFuture<Box<Changeset>, Self::Error>;
    fn get_manifest_by_nodeid(
        &self,
        nodeid: &NodeHash,
    ) -> BoxFuture<Box<Manifest<Error = Self::Error> + Sync>, Self::Error>;

    fn boxed(self) -> Box<Repo<Error = Self::Error> + Sync>
    where
        Self: Sync + Sized,
    {
        Box::new(self)
    }
}

pub struct BoxRepo<R, E>
where
    R: Repo,
{
    repo: R,
    cvterr: fn(R::Error) -> E,
    _phantom: PhantomData<E>,
}

// The box can be Sync iff R is Sync, E doesn't matter as its phantom
unsafe impl<R, E> Sync for BoxRepo<R, E>
where
    R: Repo + Sync,
{
}

impl<R, E> BoxRepo<R, E>
where
    R: Repo + Sync + Send,
    E: Send + 'static,
{
    pub fn new(repo: R) -> Box<Repo<Error = E> + Sync + Send>
    where
        E: From<R::Error>,
    {
        Self::new_with_cvterr(repo, E::from)
    }

    pub fn new_with_cvterr(
        repo: R,
        cvterr: fn(R::Error) -> E,
    ) -> Box<Repo<Error = E> + Sync + Send> {
        let br = BoxRepo {
            repo,
            cvterr,
            _phantom: PhantomData,
        };

        Box::new(br)
    }
}

impl<R, E> Repo for BoxRepo<R, E>
where
    R: Repo + Sync + Send + 'static,
    E: Send + 'static,
{
    type Error = E;

    fn get_changesets(&self) -> BoxStream<NodeHash, Self::Error> {
        self.repo.get_changesets().map_err(self.cvterr).boxed()
    }

    fn get_heads(&self) -> BoxStream<NodeHash, Self::Error> {
        self.repo.get_heads().map_err(self.cvterr).boxed()
    }

    fn get_bookmarks(&self) -> Result<BoxedBookmarks<Self::Error>, Self::Error> {
        let bookmarks = self.repo.get_bookmarks().map_err(self.cvterr)?;

        Ok(bookmarks::BoxedBookmarks::new_cvt(bookmarks, self.cvterr))
    }

    fn changeset_exists(&self, nodeid: &NodeHash) -> BoxFuture<bool, Self::Error> {
        let cvterr = self.cvterr;

        self.repo.changeset_exists(nodeid).map_err(cvterr).boxed()
    }

    fn get_changeset_by_nodeid(&self, nodeid: &NodeHash) -> BoxFuture<Box<Changeset>, Self::Error> {
        let cvterr = self.cvterr;

        self.repo
            .get_changeset_by_nodeid(nodeid)
            .map_err(cvterr)
            .boxed()
    }

    fn get_manifest_by_nodeid(
        &self,
        nodeid: &NodeHash,
    ) -> BoxFuture<Box<Manifest<Error = Self::Error> + Sync>, Self::Error> {
        let cvterr = self.cvterr;

        self.repo
            .get_manifest_by_nodeid(nodeid)
            .map(move |m| BoxManifest::new_with_cvterr(m, cvterr))
            .map_err(cvterr)
            .boxed()
    }
}


impl<RE> Repo for Box<Repo<Error = RE>>
where
    RE: Send + 'static,
{
    type Error = RE;

    fn get_changesets(&self) -> BoxStream<NodeHash, Self::Error> {
        (**self).get_changesets()
    }

    fn get_heads(&self) -> BoxStream<NodeHash, Self::Error> {
        (**self).get_heads()
    }

    fn get_bookmarks(&self) -> Result<BoxedBookmarks<Self::Error>, Self::Error> {
        (**self).get_bookmarks()
    }

    fn changeset_exists(&self, nodeid: &NodeHash) -> BoxFuture<bool, Self::Error> {
        (**self).changeset_exists(nodeid)
    }

    fn get_changeset_by_nodeid(&self, nodeid: &NodeHash) -> BoxFuture<Box<Changeset>, Self::Error> {
        (**self).get_changeset_by_nodeid(nodeid)
    }

    fn get_manifest_by_nodeid(
        &self,
        nodeid: &NodeHash,
    ) -> BoxFuture<Box<Manifest<Error = Self::Error> + Sync>, Self::Error> {
        (**self).get_manifest_by_nodeid(nodeid)
    }
}

impl<RE> Repo for Arc<Repo<Error = RE>>
where
    RE: Send + 'static,
{
    type Error = RE;

    fn get_changesets(&self) -> BoxStream<NodeHash, Self::Error> {
        (**self).get_changesets()
    }

    fn get_heads(&self) -> BoxStream<NodeHash, Self::Error> {
        (**self).get_heads()
    }

    fn get_bookmarks(&self) -> Result<BoxedBookmarks<Self::Error>, Self::Error> {
        (**self).get_bookmarks()
    }

    fn changeset_exists(&self, nodeid: &NodeHash) -> BoxFuture<bool, Self::Error> {
        (**self).changeset_exists(nodeid)
    }

    fn get_changeset_by_nodeid(&self, nodeid: &NodeHash) -> BoxFuture<Box<Changeset>, Self::Error> {
        (**self).get_changeset_by_nodeid(nodeid)
    }

    fn get_manifest_by_nodeid(
        &self,
        nodeid: &NodeHash,
    ) -> BoxFuture<Box<Manifest<Error = Self::Error> + Sync>, Self::Error> {
        (**self).get_manifest_by_nodeid(nodeid)
    }
}
