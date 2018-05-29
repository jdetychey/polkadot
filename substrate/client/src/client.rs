// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate Client

use futures::sync::mpsc;
use parking_lot::Mutex;
use std::hash;
use primitives::AuthorityId;
use runtime_primitives::{bft::Justification, generic::BlockId};
use runtime_primitives::traits::{Block as BlockT, Header as HeaderT, Zero, One};
use primitives::storage::{StorageKey, StorageData};
use codec::{KeyedVec, Slicable};
use state_machine::{self, Ext, OverlayedChanges, Backend as StateBackend, CodeExecutor};

use backend::{self, BlockImportOperation};
use blockchain::{self, Info as ChainInfo, Backend as ChainBackend};
use {error, in_mem, block_builder, runtime_io, bft};

/// Polkadot Client
pub struct Client<B, E, Block> where
	Block: BlockT,
{
	backend: B,
	executor: E,
	import_notification_sinks: Mutex<Vec<mpsc::UnboundedSender<BlockImportNotification<Block>>>>,
}

/// A source of blockchain evenets.
pub trait BlockchainEvents<Block: BlockT> {
	/// Get block import event stream.
	fn import_notification_stream(&self) -> mpsc::UnboundedReceiver<BlockImportNotification<Block>>;
}

/// Chain head information.
pub trait ChainHead<Block: BlockT> {
	/// Get best block header.
	fn best_block_header(&self) -> Result<<Block as BlockT>::Header, error::Error>;
}

/// Client info
// TODO: split queue info from chain info and amalgamate into single struct.
#[derive(Debug)]
pub struct ClientInfo<Block: BlockT> {
	/// Best block hash.
	pub chain: ChainInfo<Block>,
	/// Best block number in the queue.
	pub best_queued_number: Option<<<Block as BlockT>::Header as HeaderT>::Number>,
	/// Best queued block hash.
	pub best_queued_hash: Option<Block::Hash>,
}

/// Information regarding the result of a call.
pub struct CallResult {
	/// The data that was returned from the call.
	pub return_data: Vec<u8>,
	/// The changes made to the state by the call.
	pub changes: OverlayedChanges,
}

/// Block import result.
#[derive(Debug)]
pub enum ImportResult {
	/// Added to the import queue.
	Queued,
	/// Already in the import queue.
	AlreadyQueued,
	/// Already in the blockchain.
	AlreadyInChain,
	/// Block or parent is known to be bad.
	KnownBad,
	/// Block parent is not in the chain.
	UnknownParent,
}

/// Block status.
#[derive(Debug, PartialEq, Eq)]
pub enum BlockStatus {
	/// Added to the import queue.
	Queued,
	/// Already in the blockchain.
	InChain,
	/// Block or parent is known to be bad.
	KnownBad,
	/// Not in the queue or the blockchain.
	Unknown,
}

/// Block data origin.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BlockOrigin {
	/// Genesis block built into the client.
	Genesis,
	/// Block is part of the initial sync with the network.
	NetworkInitialSync,
	/// Block was broadcasted on the network.
	NetworkBroadcast,
	/// Block that was received from the network and validated in the consensus process.
	ConsensusBroadcast,
	/// Block that was collated by this node.
	Own,
	/// Block was imported from a file.
	File,
}

/// Summary of an imported block
#[derive(Clone, Debug)]
pub struct BlockImportNotification<Block: BlockT> {
	/// Imported block header hash.
	pub hash: Block::Hash,
	/// Imported block origin.
	pub origin: BlockOrigin,
	/// Imported block header.
	pub header: Block::Header,
	/// Is this the new best block.
	pub is_new_best: bool,
}

/// A header paired with a justification which has already been checked.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct JustifiedHeader<Block: BlockT> {
	header: <Block as BlockT>::Header,
	justification: ::bft::Justification<Block::Hash>,
}

impl<Block: BlockT> JustifiedHeader<Block> {
	/// Deconstruct the justified header into parts.
	pub fn into_inner(self) -> (<Block as BlockT>::Header, ::bft::Justification<Block::Hash>) {
		(self.header, self.justification)
	}
}

/// Create an instance of in-memory client.
pub fn new_in_mem<E, F, Block>(
	executor: E,
	build_genesis: F
) -> error::Result<Client<in_mem::Backend<Block>, E, Block>>
	where
		E: CodeExecutor,
		F: FnOnce() -> (<Block as BlockT>::Header, Vec<(Vec<u8>, Vec<u8>)>),
		Block: BlockT,
{
	Client::new(in_mem::Backend::new(), executor, build_genesis)
}

impl<B, E, Block: BlockT> Client<B, E, Block> where
	B: backend::Backend<Block>,
	E: CodeExecutor,
	Block: BlockT,
	Block::Hash: hash::Hash,
	error::Error: From<<<B as backend::Backend<Block>>::State as StateBackend>::Error>,
{
	/// Creates new Polkadot Client with given blockchain and code executor.
	pub fn new<F>(
		backend: B,
		executor: E,
		build_genesis: F
	) -> error::Result<Self>
		where
			F: FnOnce() -> (<Block as BlockT>::Header, Vec<(Vec<u8>, Vec<u8>)>)
	{
		if backend.blockchain().header(BlockId::Number(Zero::zero()))?.is_none() {
			trace!("Empty database, writing genesis block");
			let (genesis_header, genesis_store) = build_genesis();
			let mut op = backend.begin_operation(BlockId::Hash(Default::default()))?;
			op.reset_storage(genesis_store.into_iter())?;
			op.set_block_data(genesis_header, Some(vec![]), None, true)?;
			backend.commit_operation(op)?;
		}
		Ok(Client {
			backend,
			executor,
			import_notification_sinks: Mutex::new(Vec::new()),
		})
	}

	/// Get a reference to the state at a given block.
	pub fn state_at(&self, block: &BlockId<Block>) -> error::Result<B::State> {
		self.backend.state_at(*block)
	}

	/// Expose backend reference. To be used in tests only
	pub fn backend(&self) -> &B {
		&self.backend
	}

	/// Return single storage entry of contract under given address in state in a block of given hash.
	pub fn storage(&self, id: &BlockId<Block>, key: &StorageKey) -> error::Result<StorageData> {
		Ok(StorageData(self.state_at(id)?
			.storage(&key.0)?
			.ok_or_else(|| error::ErrorKind::NoValueForKey(key.0.clone()))?
			.to_vec()))
	}

	/// Get the code at a given block.
	pub fn code_at(&self, id: &BlockId<Block>) -> error::Result<Vec<u8>> {
		self.storage(id, &StorageKey(b":code".to_vec())).map(|data| data.0)
	}

	/// Clone a new instance of Executor.
	pub fn clone_executor(&self) -> E where E: Clone {
		self.executor.clone()
	}

	/// Get the current set of authorities from storage.
	pub fn authorities_at(&self, id: &BlockId<Block>) -> error::Result<Vec<AuthorityId>> {
		let state = self.state_at(id)?;
		(0..u32::decode(&mut state.storage(b":auth:len")?.ok_or(error::ErrorKind::AuthLenEmpty)?.as_slice()).ok_or(error::ErrorKind::AuthLenInvalid)?)
			.map(|i| state.storage(&i.to_keyed_vec(b":auth:"))
				.map_err(|e| error::Error::from(e).into())
				.and_then(|v| v.ok_or(error::ErrorKind::AuthEmpty(i)))
				.and_then(|s| AuthorityId::decode(&mut s.as_slice()).ok_or(error::ErrorKind::AuthInvalid(i)))
				.map_err(Into::into)
			).collect()
	}

	/// Execute a call to a contract on top of state in a block of given hash.
	///
	/// No changes are made.
	pub fn call(&self, id: &BlockId<Block>, method: &str, call_data: &[u8]) -> error::Result<CallResult> {
		let mut changes = OverlayedChanges::default();
		let (return_data, _) = state_machine::execute(
			&self.state_at(id)?,
			&mut changes,
			&self.executor,
			method,
			call_data,
		)?;
		Ok(CallResult { return_data, changes })
	}

	/// Set up the native execution environment to call into a native runtime code.
	pub fn using_environment<F: FnOnce() -> T, T>(
		&self, f: F
	) -> error::Result<T> {
		self.using_environment_at(&BlockId::Number(self.info()?.chain.best_number), &mut Default::default(), f)
	}

	/// Set up the native execution environment to call into a native runtime code.
	pub fn using_environment_at<F: FnOnce() -> T, T>(
		&self,
		id: &BlockId<Block>,
		overlay: &mut OverlayedChanges,
		f: F
	) -> error::Result<T> {
		Ok(runtime_io::with_externalities(&mut Ext::new(overlay, &self.state_at(id)?), f))
	}

	/// Create a new block, built on the head of the chain.
	pub fn new_block(&self) -> error::Result<block_builder::BlockBuilder<B, E, Block>> where E: Clone {
		block_builder::BlockBuilder::new(self)
	}

	/// Create a new block, built on top of `parent`.
	pub fn new_block_at(&self, parent: &BlockId<Block>) -> error::Result<block_builder::BlockBuilder<B, E, Block>> where E: Clone {
		block_builder::BlockBuilder::at_block(parent, &self)
	}

	/// Check a header's justification.
	pub fn check_justification(
		&self,
		header: <Block as BlockT>::Header,
		justification: ::bft::UncheckedJustification<Block::Hash>,
	) -> error::Result<JustifiedHeader<Block>> {
		let parent_hash = header.parent_hash().clone();
		let authorities = self.authorities_at(&BlockId::Hash(parent_hash))?;
		let just = ::bft::check_justification::<Block>(&authorities[..], parent_hash, justification)
			.map_err(|_|
				error::ErrorKind::BadJustification(
					format!("{}", header.hash())
				)
			)?;
		Ok(JustifiedHeader {
			header,
			justification: just,
		})
	}

	/// Queue a block for import.
	pub fn import_block(
		&self,
		origin: BlockOrigin,
		header: JustifiedHeader<Block>,
		body: Option<Vec<<Block as BlockT>::Extrinsic>>,
	) -> error::Result<ImportResult> {
		// TODO: import lock
		// TODO: import justification.
		let (header, justification) = header.into_inner();
		let parent_hash = header.parent_hash().clone();
		match self.backend.blockchain().status(BlockId::Hash(parent_hash))? {
			blockchain::BlockStatus::InChain => (),
			blockchain::BlockStatus::Unknown => return Ok(ImportResult::UnknownParent),
		}

		let mut transaction = self.backend.begin_operation(BlockId::Hash(parent_hash))?;
		let mut overlay = OverlayedChanges::default();

		let (_out, storage_update) = state_machine::execute(
			transaction.state()?,
			&mut overlay,
			&self.executor,
			"execute_block",
			&<Block as BlockT>::new(header.clone(), body.clone().unwrap_or_default()).encode()
		)?;

		let is_new_best = header.number() == &(self.backend.blockchain().info()?.best_number + One::one());
		let hash = header.hash();
		trace!("Imported {}, (#{}), best={}, origin={:?}", hash, header.number(), is_new_best, origin);
		transaction.set_block_data(header.clone(), body, Some(justification.uncheck().into()), is_new_best)?;
		transaction.update_storage(storage_update)?;
		self.backend.commit_operation(transaction)?;

		if origin == BlockOrigin::NetworkBroadcast || origin == BlockOrigin::Own || origin == BlockOrigin::ConsensusBroadcast {
			let notification = BlockImportNotification::<Block> {
				hash: hash,
				origin: origin,
				header: header,
				is_new_best: is_new_best,
			};
			self.import_notification_sinks.lock().retain(|sink| !sink.unbounded_send(notification.clone()).is_err());
		}

		Ok(ImportResult::Queued)
	}

	/// Get blockchain info.
	pub fn info(&self) -> error::Result<ClientInfo<Block>> {
		let info = self.backend.blockchain().info().map_err(|e| error::Error::from_blockchain(Box::new(e)))?;
		Ok(ClientInfo {
			chain: info,
			best_queued_hash: None,
			best_queued_number: None,
		})
	}

	/// Get block status.
	pub fn block_status(&self, id: &BlockId<Block>) -> error::Result<BlockStatus> {
		// TODO: more efficient implementation
		match self.backend.blockchain().header(*id).map_err(|e| error::Error::from_blockchain(Box::new(e)))?.is_some() {
			true => Ok(BlockStatus::InChain),
			false => Ok(BlockStatus::Unknown),
		}
	}

	/// Get block hash by number.
	pub fn block_hash(&self, block_number: <<Block as BlockT>::Header as HeaderT>::Number) -> error::Result<Option<Block::Hash>> {
		self.backend.blockchain().hash(block_number)
	}

	/// Convert an arbitrary block ID into a block hash.
	pub fn block_hash_from_id(&self, id: &BlockId<Block>) -> error::Result<Option<Block::Hash>> {
		match *id {
			BlockId::Hash(h) => Ok(Some(h)),
			BlockId::Number(n) => self.block_hash(n),
		}
	}

	/// Convert an arbitrary block ID into a block hash.
	pub fn block_number_from_id(&self, id: &BlockId<Block>) -> error::Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>> {
		match *id {
			BlockId::Hash(_) => Ok(self.header(id)?.map(|h| h.number().clone())),
			BlockId::Number(n) => Ok(Some(n)),
		}
	}

	/// Get block header by id.
	pub fn header(&self, id: &BlockId<Block>) -> error::Result<Option<<Block as BlockT>::Header>> {
		self.backend.blockchain().header(*id)
	}

	/// Get block body by id.
	pub fn body(&self, id: &BlockId<Block>) -> error::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		self.backend.blockchain().body(*id)
	}

	/// Get block justification set by id.
	pub fn justification(&self, id: &BlockId<Block>) -> error::Result<Option<Justification<Block::Hash>>> {
		self.backend.blockchain().justification(*id)
	}

	/// Get best block header.
	pub fn best_block_header(&self) -> error::Result<<Block as BlockT>::Header> {
		let info = self.backend.blockchain().info().map_err(|e| error::Error::from_blockchain(Box::new(e)))?;
		Ok(self.header(&BlockId::Hash(info.best_hash))?.expect("Best block header must always exist"))
	}
}

impl<B, E, Block> bft::BlockImport<Block> for Client<B, E, Block>
	where
		B: backend::Backend<Block>,
		E: state_machine::CodeExecutor,
		Block: BlockT,
		Block::Hash: hash::Hash,
		error::Error: From<<B::State as state_machine::backend::Backend>::Error>
{
	fn import_block(&self, block: Block, justification: ::bft::Justification<Block::Hash>) {
		let (header, extrinsics) = block.deconstruct();
		let justified_header = JustifiedHeader {
			header: header,
			justification,
		};

		let _ = self.import_block(BlockOrigin::ConsensusBroadcast, justified_header, Some(extrinsics));
	}
}

impl<B, E, Block> bft::Authorities<Block> for Client<B, E, Block>
	where
		B: backend::Backend<Block>,
		E: state_machine::CodeExecutor,
		Block: BlockT,
		Block::Hash: hash::Hash,
		error::Error: From<<B::State as state_machine::backend::Backend>::Error>,
{
	fn authorities(&self, at: &BlockId<Block>) -> Result<Vec<AuthorityId>, bft::Error> {
		self.authorities_at(at).map_err(|_| {
			let descriptor = format!("{:?}", at);
			bft::ErrorKind::StateUnavailable(descriptor).into()
		})
	}
}

impl<B, E, Block> BlockchainEvents<Block> for Client<B, E, Block>
	where
		B: backend::Backend<Block>,
		E: state_machine::CodeExecutor,
		Block: BlockT,
		error::Error: From<<B::State as state_machine::backend::Backend>::Error>
{
	/// Get block import event stream.
	fn import_notification_stream(&self) -> mpsc::UnboundedReceiver<BlockImportNotification<Block>> {
		let (sink, stream) = mpsc::unbounded();
		self.import_notification_sinks.lock().push(sink);
		stream
	}
}

impl<B, E, Block> ChainHead<Block> for Client<B, E, Block>
	where
		B: backend::Backend<Block>,
		E: state_machine::CodeExecutor,
		Block: BlockT,
		error::Error: From<<B::State as state_machine::backend::Backend>::Error>
{
	fn best_block_header(&self) -> error::Result<<Block as BlockT>::Header> {
		Client::best_block_header(self)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use codec::Slicable;
	use keyring::Keyring;
	use test_client::{self, TestClient};
	use test_client::client::BlockOrigin;
	use test_client::runtime as test_runtime;
	use test_client::runtime::{UncheckedTransaction, Transaction};

	#[test]
	fn client_initialises_from_genesis_ok() {
		let client = test_client::new();
		let genesis_hash = client.block_hash(0).unwrap().unwrap();

		assert_eq!(client.using_environment(|| test_runtime::system::latest_block_hash()).unwrap(), genesis_hash);
		assert_eq!(client.using_environment(|| test_runtime::system::balance_of(Keyring::Alice.to_raw_public())).unwrap(), 1000);
		assert_eq!(client.using_environment(|| test_runtime::system::balance_of(Keyring::Ferdie.to_raw_public())).unwrap(), 0);
	}

	#[test]
	fn authorities_call_works() {
		let client = test_client::new();

		assert_eq!(client.info().unwrap().chain.best_number, 0);
		assert_eq!(client.authorities_at(&BlockId::Number(0)).unwrap(), vec![
			Keyring::Alice.to_raw_public(),
			Keyring::Bob.to_raw_public(),
			Keyring::Charlie.to_raw_public()
		]);
	}

	#[test]
	fn block_builder_works_with_no_transactions() {
		let client = test_client::new();

		let builder = client.new_block().unwrap();

		client.justify_and_import(BlockOrigin::Own, builder.bake().unwrap()).unwrap();

		assert_eq!(client.info().unwrap().chain.best_number, 1);
		assert_eq!(client.using_environment(|| test_runtime::system::latest_block_hash()).unwrap(), client.block_hash(1).unwrap().unwrap());
	}

	fn sign_tx(tx: Transaction) -> UncheckedTransaction {
		let signature = Keyring::from_raw_public(tx.from.clone()).unwrap().sign(&tx.encode());
		UncheckedTransaction { tx, signature }
	}

	#[test]
	fn block_builder_works_with_transactions() {
		let client = test_client::new();

		let mut builder = client.new_block().unwrap();

		builder.push(sign_tx(Transaction {
			from: Keyring::Alice.to_raw_public(),
			to: Keyring::Ferdie.to_raw_public(),
			amount: 42,
			nonce: 0
		})).unwrap();

		client.justify_and_import(BlockOrigin::Own, builder.bake().unwrap()).unwrap();

		assert_eq!(client.info().unwrap().chain.best_number, 1);
		assert!(client.state_at(&BlockId::Number(1)).unwrap() != client.state_at(&BlockId::Number(0)).unwrap());
		assert_eq!(client.using_environment(|| test_runtime::system::balance_of(Keyring::Alice.to_raw_public())).unwrap(), 958);
		assert_eq!(client.using_environment(|| test_runtime::system::balance_of(Keyring::Ferdie.to_raw_public())).unwrap(), 42);
	}
}
