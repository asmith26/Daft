use std::sync::Arc;

use async_trait::async_trait;
use common_error::{DaftError, DaftResult};

use crate::{
    compute::{
        partition::{virtual_partition::VirtualPartitionSet, PartitionRef},
        tree::PartitionTaskNode,
    },
    executor::Executor,
};

use super::{channel::OutputChannel, streaming::StreamingPartitionTaskScheduler};

#[derive(Debug)]
pub struct BulkPartitionTaskScheduler<T: PartitionRef, E: Executor<T>> {
    task_tree_root: PartitionTaskNode,
    leaf_inputs: Vec<VirtualPartitionSet<T>>,
    max_output_queue_size: Option<usize>,
    executor: Arc<E>,
}

impl<T: PartitionRef, E: Executor<T>> BulkPartitionTaskScheduler<T, E> {
    pub fn new(
        task_tree_root: PartitionTaskNode,
        leaf_inputs: Vec<VirtualPartitionSet<T>>,
        max_output_queue_size: Option<usize>,
        executor: Arc<E>,
    ) -> Self {
        Self {
            task_tree_root,
            leaf_inputs,
            max_output_queue_size,
            executor,
        }
    }

    pub async fn execute(self) -> DaftResult<Vec<Vec<T>>> {
        // Delegate to streaming scheduler, materializing all results in the output channel into a bulk vec.
        // TODO(Clark): When the need arises, create a dedicated bulk scheduler that optimizes for bulk materialization.
        let mut output = Ok(std::iter::repeat_with(|| Vec::new())
            .take(self.task_tree_root.num_outputs())
            .collect());
        let output_channel = SendToVec::new(&mut output);
        let streaming_scheduler = StreamingPartitionTaskScheduler::new(
            self.task_tree_root,
            self.leaf_inputs,
            output_channel,
            self.max_output_queue_size,
            self.executor,
        );
        streaming_scheduler.execute().await;
        output
    }
}

#[derive(Debug)]
pub struct SendToVec<'a, T: PartitionRef> {
    out: &'a mut DaftResult<Vec<Vec<T>>>,
}

impl<'a, T: PartitionRef> SendToVec<'a, T> {
    pub fn new(out: &'a mut DaftResult<Vec<Vec<T>>>) -> Self {
        Self { out }
    }
}

#[async_trait(?Send)]
impl<'a, T: PartitionRef> OutputChannel<T> for SendToVec<'a, T> {
    async fn send_output(&mut self, output: DaftResult<Vec<T>>) -> DaftResult<()> {
        match output {
            Ok(value) => self
                .out
                .as_mut()
                .map(|values| {
                    values
                        .iter_mut()
                        .zip(value.into_iter())
                        .for_each(|(lane, v)| lane.push(v))
                })
                .map_err(|_| {
                    DaftError::InternalError("Receiver dropped before done sending".to_string())
                }),
            Err(e) => {
                *self.out = Err(e);
                Err(DaftError::InternalError(
                    "Receiver dropped before done sending".to_string(),
                ))
            }
        }
    }
}
