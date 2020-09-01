// Copyright 2020. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::convert::{TryFrom, TryInto};
use tari_core::{
    proto::utils::try_convert_all,
    transactions::{
        aggregated_body::AggregateBody,
        bullet_rangeproofs::BulletRangeProof,
        tari_amount::MicroTari,
        transaction::{KernelFeatures, TransactionKernel},
        types::{Commitment, PrivateKey, PublicKey, Signature},
        OutputFeatures,
        OutputFlags,
        TransactionInput,
        TransactionOutput,
    },
};
use tari_crypto::tari_utilities::ByteArray;

use crate::tari_grpc::base_node_grpc as grpc;

impl TryFrom<grpc::AggregateBody> for AggregateBody {
    type Error = String;

    fn try_from(body: grpc::AggregateBody) -> Result<Self, Self::Error> {
        let inputs = try_convert_all(body.inputs)?;
        let outputs = try_convert_all(body.outputs)?;
        let kernels = try_convert_all(body.kernels)?;
        let mut body = AggregateBody::new(inputs, outputs, kernels);
        body.sort();
        Ok(body)
    }
}

impl TryFrom<grpc::TransactionOutput> for TransactionOutput {
    type Error = String;

    fn try_from(output: grpc::TransactionOutput) -> Result<Self, Self::Error> {
        let features = output
            .features
            .map(TryInto::try_into)
            .ok_or_else(|| "transaction output features not provided".to_string())??;

        let commitment = Commitment::from_bytes(&output.commitment).map_err(|err| err.to_string())?;
        Ok(TransactionOutput::new(
            features,
            commitment,
            BulletRangeProof(output.range_proof),
            &output.script_hash,
        ))
    }
}

impl TryFrom<grpc::TransactionKernel> for TransactionKernel {
    type Error = String;

    fn try_from(kernel: grpc::TransactionKernel) -> Result<Self, Self::Error> {
        let excess = Commitment::from_bytes(&kernel.excess).map_err(|err| err.to_string())?;

        let excess_sig = kernel
            .excess_sig
            .ok_or_else(|| "excess_sig not provided".to_string())?
            .try_into()
            .map_err(|_| "excess_sig could not be converted".to_string())?;

        Ok(Self {
            features: KernelFeatures::from_bits(kernel.features as u8)
                .ok_or_else(|| "Invalid or unrecognised kernel feature flag".to_string())?,
            excess,
            excess_sig,
            fee: MicroTari::from(kernel.fee),
            linked_kernel: Some(kernel.linked_kernel),
            lock_height: kernel.lock_height,
            meta_info: Some(kernel.meta_info),
        })
    }
}
impl TryFrom<grpc::Signature> for Signature {
    type Error = String;

    fn try_from(sig: grpc::Signature) -> Result<Self, Self::Error> {
        let public_nonce =
            PublicKey::from_bytes(&sig.public_nonce).map_err(|_| "Could not get public nonce".to_string())?;
        let signature = PrivateKey::from_bytes(&sig.signature).map_err(|_| "Could not get signature".to_string())?;

        Ok(Self::new(public_nonce, signature))
    }
}

impl TryFrom<grpc::TransactionInput> for TransactionInput {
    type Error = String;

    fn try_from(input: grpc::TransactionInput) -> Result<Self, Self::Error> {
        let features = input
            .features
            .map(TryInto::try_into)
            .ok_or_else(|| "transaction output features not provided".to_string())??;

        let commitment = Commitment::from_bytes(&input.commitment).map_err(|err| err.to_string())?;
        Ok(TransactionInput::new(features, commitment, &input.script_hash))
    }
}

impl TryFrom<grpc::OutputFeatures> for OutputFeatures {
    type Error = String;

    fn try_from(features: grpc::OutputFeatures) -> Result<Self, Self::Error> {
        Ok(Self {
            flags: OutputFlags::from_bits(features.flags as u8)
                .ok_or_else(|| "Invalid or unrecognised output flags".to_string())?,
            maturity: features.maturity,
        })
    }
}
