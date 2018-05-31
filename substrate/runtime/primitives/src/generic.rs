// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate Demo.

// Substrate Demo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate Demo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate Demo.  If not, see <http://www.gnu.org/licenses/>.

//! Generic implementations of Extrinsic/Header/Block.

#[cfg(feature = "std")]
use std::fmt;

#[cfg(feature = "std")]
use serde::{Deserialize, Deserializer};

use rstd::prelude::*;
use codec::{Slicable, Input};
use runtime_support::AuxDispatchable;
use traits::{self, Member, SimpleArithmetic, SimpleBitOps, MaybeDisplay, Block as BlockT,
	Header as HeaderT, Hashing as HashingT};
use rstd::ops;

/// A vetted and verified extrinsic from the external world.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Extrinsic<AccountId, Index, Call> {
	/// Who signed it (note this is not a signature).
	pub signed: AccountId,
	/// The number of extrinsics have come before from the same signer.
	pub index: Index,
	/// The function that should be called.
	pub function: Call,
}

impl<AccountId, Index, Call> Slicable for Extrinsic<AccountId, Index, Call> where
 	AccountId: Member + Slicable + MaybeDisplay,
 	Index: Member + Slicable + MaybeDisplay + SimpleArithmetic,
 	Call: Member + Slicable
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Extrinsic {
			signed: Slicable::decode(input)?,
			index: Slicable::decode(input)?,
			function: Slicable::decode(input)?,
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		self.signed.using_encoded(|s| v.extend(s));
		self.index.using_encoded(|s| v.extend(s));
		self.function.using_encoded(|s| v.extend(s));

		v
	}
}

/// A extrinsics right from the external world. Unchecked.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct UncheckedExtrinsic<AccountId, Index, Call, Signature> {
	/// The actual extrinsic information.
	pub extrinsic: Extrinsic<AccountId, Index, Call>,
	/// The signature; should be an Ed25519 signature applied to the serialised `extrinsic` field.
	pub signature: Signature,
}

impl<AccountId, Index, Call, Signature> traits::Checkable for UncheckedExtrinsic<AccountId, Index, Call, Signature> where
 	AccountId: Member + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member + traits::Verify<Signer = AccountId>,
	Extrinsic<AccountId, Index, Call>: Slicable,
{
	type Checked = CheckedExtrinsic<AccountId, Index, Call, Signature>;

	fn check(self) -> Result<Self::Checked, Self> {
		// TODO: unfortunately this is a lifetime relationship that can't
		// be expressed without higher-kinded lifetimes.
		struct LazyEncode<F> {
			inner: F,
			encoded: Option<Vec<u8>>,
		}

		impl<F: Fn() -> Vec<u8>> traits::Lazy<[u8]> for LazyEncode<F> {
			fn get(&mut self) -> &[u8] {
				self.encoded.get_or_insert_with(&self.inner).as_slice()
			}
		}

		let sig_ok = {
			self.signature.verify(
				LazyEncode { inner: || self.extrinsic.encode(), encoded: None },
				&self.extrinsic.signed,
			)
		};

		if sig_ok {
			Ok(CheckedExtrinsic(self))
		} else {
			Err(self)
		}
	}
}

impl<AccountId, Index, Call, Signature> UncheckedExtrinsic<AccountId, Index, Call, ::MaybeUnsigned<Signature>> where
 	AccountId: Member + Default + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member + Default + traits::Verify<Signer = AccountId>,
	Extrinsic<AccountId, Index, Call>: Slicable,
{
	/// Is this extrinsic signed?
	pub fn is_signed(&self) -> bool {
		self.signature.is_signed(&self.extrinsic.signed)
	}
}

impl<AccountId, Index, Call, Signature> Slicable for UncheckedExtrinsic<AccountId, Index, Call, Signature> where
 	AccountId: Member + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member + Slicable,
	Extrinsic<AccountId, Index, Call>: Slicable,
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		// This is a little more complicated than usual since the binary format must be compatible
		// with substrate's generic `Vec<u8>` type. Basically this just means accepting that there
		// will be a prefix of u32, which has the total number of bytes following (we don't need
		// to use this).
		let _length_do_not_remove_me_see_above: u32 = Slicable::decode(input)?;

		Some(UncheckedExtrinsic {
			extrinsic: Slicable::decode(input)?,
			signature: Slicable::decode(input)?,
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		// need to prefix with the total length as u32 to ensure it's binary comptible with
		// Vec<u8>. we'll make room for it here, then overwrite once we know the length.
		v.extend(&[0u8; 4]);

/*		self.extrinsic.signed.using_encoded(|s| v.extend(s));
		self.extrinsic.index.using_encoded(|s| v.extend(s));
		self.extrinsic.function.using_encoded(|s| v.extend(s));*/
		self.extrinsic.using_encoded(|s| v.extend(s));

		self.signature.using_encoded(|s| v.extend(s));

		let length = (v.len() - 4) as u32;
		length.using_encoded(|s| v[0..4].copy_from_slice(s));

		v
	}
}

#[cfg(feature = "std")]
impl<AccountId, Index, Call, Signature> fmt::Debug for UncheckedExtrinsic<AccountId, Index, Call, Signature> where
 	AccountId: fmt::Debug,
 	Index: fmt::Debug,
 	Call: fmt::Debug,
{
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "UncheckedExtrinsic({:?})", self.extrinsic)
	}
}

/// A type-safe indicator that a extrinsic has been checked.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct CheckedExtrinsic<AccountId, Index, Call, Signature>
	(UncheckedExtrinsic<AccountId, Index, Call, Signature>);

impl<AccountId, Index, Call, Signature> CheckedExtrinsic<AccountId, Index, Call, Signature>
where
 	AccountId: Member + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member
{
	/// Get a reference to the checked signature.
	pub fn signature(&self) -> &Signature {
		&self.0.signature
	}

	/// Get a reference to the checked signature.
	pub fn as_unchecked(&self) -> &UncheckedExtrinsic<AccountId, Index, Call, Signature> {
		&self.0
	}

	/// Get a reference to the checked signature.
	pub fn into_unchecked(self) -> UncheckedExtrinsic<AccountId, Index, Call, Signature> {
		self.0
	}
}

impl<AccountId, Index, Call, Signature> ops::Deref
	for CheckedExtrinsic<AccountId, Index, Call, Signature>
where
 	AccountId: Member + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member
{
	type Target = Extrinsic<AccountId, Index, Call>;

	fn deref(&self) -> &Self::Target {
		&self.0.extrinsic
	}
}

impl<AccountId, Index, Call, Signature> traits::Applyable
	for CheckedExtrinsic<AccountId, Index, Call, Signature>
where
 	AccountId: Member + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member + AuxDispatchable<Aux = AccountId>,
	Signature: Member
{
	type Index = Index;
	type AccountId = AccountId;

	fn index(&self) -> &Self::Index {
		&self.0.extrinsic.index
	}

	fn sender(&self) -> &Self::AccountId {
		&self.0.extrinsic.signed
	}

	fn apply(self) {
		let xt = self.0.extrinsic;
		xt.function.dispatch(&xt.signed);
	}
}

#[derive(Default, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
pub struct Digest<Item> {
	pub logs: Vec<Item>,
}

impl<Item> Slicable for Digest<Item> where
 	Item: Member + Default + Slicable
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Digest { logs: Slicable::decode(input)? })
	}
	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		self.logs.using_encoded(f)
	}
}
impl<Item> traits::Digest for Digest<Item> where
 	Item: Member + Default + Slicable
{
	type Item = Item;
	fn push(&mut self, item: Self::Item) {
		self.logs.push(item);
	}
}


/// Abstraction over a block header for a substrate chain.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Serialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct Header<Number, Hashing: HashingT, DigestItem> {
	/// The parent hash.
	pub parent_hash: <Hashing as HashingT>::Output,
	/// The block number.
	pub number: Number,
	/// The state trie merkle root
	pub state_root: <Hashing as HashingT>::Output,
	/// The merkle root of the extrinsics.
	pub extrinsics_root: <Hashing as HashingT>::Output,
	/// A chain-specific digest of data useful for light clients or referencing auxiliary data.
	pub digest: Digest<DigestItem>,
}

// Hack to work around the fact that deriving deserialize doesn't work nicely with
// the `hashing` trait without requiring that it itself is deserializable.

// dummy struct that uses the hash type directly.
#[cfg(feature = "std")]
#[derive(Deserialize)]
struct DeserializeHeader<N, H, D> {
	parent_hash: H,
	number: N,
	state_root: H,
	extrinsics_root: H,
	digest: Digest<D>,
}

impl<N, D, Hashing: HashingT> From<DeserializeHeader<N, Hashing::Output, D>> for Header<N, Hashing, D> {
	fn from(other: DeserializeHeader<N, Hashing::Output, D>) -> Self {
		Header {
			parent_hash: other.parent_hash,
			number: other.number,
			state_root: other.state_root,
			extrinsics_root: other.extrinsics_root,
			digest: other.digest,
		}
	}
}

#[cfg(feature = "std")]
impl<'a, Number: 'a, Hashing: 'a + HashingT, DigestItem: 'a> Deserialize<'a> for Header<Number, Hashing, DigestItem> where
	Number: Deserialize<'a>,
	Hashing::Output: Deserialize<'a>,
	DigestItem: Deserialize<'a>,
{
	fn deserialize<D: Deserializer<'a>>(de: D) -> Result<Self, D::Error> {
		DeserializeHeader::<Number, Hashing::Output, DigestItem>::deserialize(de).map(Into::into)
	}
}

impl<Number, Hashing, DigestItem> Slicable for Header<Number, Hashing, DigestItem> where
	Number: Member + Slicable + MaybeDisplay + SimpleArithmetic + Slicable,
	Hashing: HashingT,
	DigestItem: Member + Default + Slicable,
	Hashing::Output: Default + Member + MaybeDisplay + SimpleBitOps + Slicable,
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Header {
			parent_hash: Slicable::decode(input)?,
			number: Slicable::decode(input)?,
			state_root: Slicable::decode(input)?,
			extrinsics_root: Slicable::decode(input)?,
			digest: Slicable::decode(input)?,
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();
		self.parent_hash.using_encoded(|s| v.extend(s));
		self.number.using_encoded(|s| v.extend(s));
		self.state_root.using_encoded(|s| v.extend(s));
		self.extrinsics_root.using_encoded(|s| v.extend(s));
		self.digest.using_encoded(|s| v.extend(s));
		v
	}
}
impl<Number, Hashing, DigestItem> traits::Header for Header<Number, Hashing, DigestItem> where
 	Number: Member + ::rstd::hash::Hash + Copy + Slicable + MaybeDisplay + SimpleArithmetic + Slicable,
	Hashing: HashingT,
	DigestItem: Member + Default + Slicable,
	Hashing::Output: Default + ::rstd::hash::Hash + Copy + Member + MaybeDisplay + SimpleBitOps + Slicable,
 {
	type Number = Number;
	type Hash = <Hashing as HashingT>::Output;
	type Hashing = Hashing;
	type Digest = Digest<DigestItem>;

	fn number(&self) -> &Self::Number { &self.number }
	fn set_number(&mut self, num: Self::Number) { self.number = num }

	fn extrinsics_root(&self) -> &Self::Hash { &self.extrinsics_root }
	fn set_extrinsics_root(&mut self, root: Self::Hash) { self.extrinsics_root = root }

	fn state_root(&self) -> &Self::Hash { &self.state_root }
	fn set_state_root(&mut self, root: Self::Hash) { self.state_root = root }

	fn parent_hash(&self) -> &Self::Hash { &self.parent_hash }
	fn set_parent_hash(&mut self, hash: Self::Hash) { self.parent_hash = hash }

	fn digest(&self) -> &Self::Digest { &self.digest }
	fn set_digest(&mut self, digest: Self::Digest) { self.digest = digest }

	fn new(
		number: Self::Number,
		extrinsics_root: Self::Hash,
		state_root: Self::Hash,
		parent_hash: Self::Hash,
		digest: Self::Digest
	) -> Self {
		Header {
			number, extrinsics_root: extrinsics_root, state_root, parent_hash, digest
		}
	}
}

/// Something to identify a block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Serialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub enum BlockId<Block: BlockT> where {
	/// Identify by block header hash.
	Hash(<<Block as BlockT>::Header as HeaderT>::Hash),
	/// Identify by block number.
	Number(<<Block as BlockT>::Header as HeaderT>::Number),
}

impl<Block: BlockT> Copy for BlockId<Block> {}

#[cfg(feature = "std")]
impl<Block: BlockT> fmt::Display for BlockId<Block> {
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		write!(f, "{:?}", self)
	}
}

/// Abstraction over a substrate block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Serialize))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct Block<Number, Hashing: HashingT, DigestItem, AccountId, Index, Call, Signature> {
	/// The block header.
	pub header: Header<Number, Hashing, DigestItem>,
	/// The accompanying extrinsics.
	pub extrinsics: Vec<UncheckedExtrinsic<AccountId, Index, Call, Signature>>,
}

// Hack to work around the fact that deriving deserialize doesn't work nicely with
// the `hashing` trait without requiring that it itself is deserializable.
#[cfg(feature = "std")]
impl<'a, Number, Hashing: HashingT, DigestItem, AccountId, Index, Call, Signature> Deserialize<'a> for Block<Number, Hashing, DigestItem, AccountId, Index, Call, Signature> where
	Number: 'a + Deserialize<'a>,
	Hashing::Output: 'a + Deserialize<'a>,
	DigestItem: 'a + Deserialize<'a>,
	AccountId: 'a + Deserialize<'a>,
	Index: 'a + Deserialize<'a>,
	Call: 'a + Deserialize<'a>,
	Signature: 'a + Deserialize<'a>,
{
	fn deserialize<D: Deserializer<'a>>(de: D) -> Result<Self, D::Error> {
		// dummy struct that uses the hash type directly.
		#[derive(Deserialize)]
		struct Inner<N, H, D, X> {
			header: DeserializeHeader<N, H, D>,
			extrinsics: Vec<X>,
		}

		let inner = Inner::<
			Number,
			Hashing::Output,
			DigestItem,
			UncheckedExtrinsic<AccountId, Index, Call, Signature>,
		>::deserialize(de)?;

		Ok(Block {
			header: inner.header.into(),
			extrinsics: inner.extrinsics,
		})
	}
}

impl<Number, Hashing, DigestItem, AccountId, Index, Call, Signature> Slicable
	for Block<Number, Hashing, DigestItem, AccountId, Index, Call, Signature>
where
	Number: Member + MaybeDisplay + SimpleArithmetic + Slicable,
	Hashing: HashingT,
	DigestItem: Member + Default,
	<Hashing as HashingT>::Output: Default + Member + MaybeDisplay + SimpleBitOps + Slicable,
 	AccountId: Member + Default + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member + Default + traits::Verify<Signer = AccountId>,
	Header<Number, Hashing, DigestItem>: traits::Header,
	UncheckedExtrinsic<AccountId, Index, Call, Signature>: Slicable,
	Extrinsic<AccountId, Index, Call>: Slicable,
{
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Block {
			header: Slicable::decode(input)?,
			extrinsics: Slicable::decode(input)?,
		})
	}
	fn encode(&self) -> Vec<u8> {
		let mut v: Vec<u8> = Vec::new();
		v.extend(self.header.encode());
		v.extend(self.extrinsics.encode());
		v
	}
}

impl<Number, Hashing, DigestItem, AccountId, Index, Call, Signature> traits::Block
	for Block<Number, Hashing, DigestItem, AccountId, Index, Call, Signature>
where
	Number: Member + MaybeDisplay + SimpleArithmetic + Slicable,
	Hashing: HashingT,
	Hashing::Output: ::rstd::hash::Hash,
	DigestItem: Member + Default,
	<Hashing as HashingT>::Output: Default + Member + MaybeDisplay + SimpleBitOps + Slicable,
 	AccountId: Member + Default + MaybeDisplay,
 	Index: Member + MaybeDisplay + SimpleArithmetic,
 	Call: Member,
	Signature: Member + Default + traits::Verify<Signer = AccountId>,
	Header<Number, Hashing, DigestItem>: traits::Header,
	UncheckedExtrinsic<AccountId, Index, Call, Signature>: Slicable,
	Extrinsic<AccountId, Index, Call>: Slicable,
{
	type Extrinsic = UncheckedExtrinsic<AccountId, Index, Call, Signature>;
	type Header = Header<Number, Hashing, DigestItem>;
	type Hash = <Self::Header as traits::Header>::Hash;

	fn header(&self) -> &Self::Header {
		&self.header
	}
	fn extrinsics(&self) -> &[Self::Extrinsic] {
		&self.extrinsics[..]
	}
	fn deconstruct(self) -> (Self::Header, Vec<Self::Extrinsic>) {
		(self.header, self.extrinsics)
	}
	fn new(header: Self::Header, extrinsics: Vec<Self::Extrinsic>) -> Self {
		Block { header, extrinsics }
	}
}
