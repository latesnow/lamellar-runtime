use crate::array::*;

#[doc(alias("One-sided", "onesided"))]
/// The interface for remotely reading elements
///
/// These operations can be performed using any LamellarArray type.
///
/// Both single element operations and batched element operations are provided
///
/// Generally if you are performing a large number of operations it will be better to
/// use a batched version instead of multiple single element opertations. While the
/// Runtime internally performs message aggregation for both single element and batched
/// operations, single element operations have to be treated as individual requests, resulting
/// in allocation and bookkeeping overheads. A single batched call on the other hand is treated
/// as a single request by the runtime.
///
/// The results of a batched operation are returned to the user in the same order as the input indices.
///
/// # One-sided Operation
/// performing either single or batched operations are both one-sided, with the calling PE performing any necessary work to
/// initate and execute active messages that are sent to remote PEs.
/// For Ops that return results, the result will only be available on the calling PE.
///
/// # Note
/// For both single index and batched operations there are no guarantees to the order in which individual operations occur
///
/// # Examples
///```
/// use lamellar::array::prelude::*;
/// use futures::future::join_all;
///
/// let world = LamellarWorldBuilder::new().build();
/// let array = AtomicArray::<usize>::new(&world,100,Distribution::Block);
///
/// let indices = vec![3,54,12,88,29,68];
/// let reqs = indices.iter().map(|i| array.load(*i)).collect::<Vec<_>>();
/// let vals_1 = array.block_on(async move {
///     // reqs.into_iter().map(|req| req.await).collect::<Vec<_>>()
///     join_all(reqs).await
/// });
/// let req = array.batch_load(indices);
/// let vals_2 = array.block_on(req);
/// for (v1,v2) in vals_1.iter().zip(vals_2.iter()){
///     assert_eq!(v1,v2);
/// }
///```
pub trait ReadOnlyOps<T: ElementOps>: private::LamellarArrayPrivate<T> {
    /// This call returns the value of the element at the specified index
    ///
    /// A future is returned as the result of this call, which is used to retrieve
    /// the result after the (possibly remote) operation as finished.
    ///
    /// # Note
    /// This future is only lazy with respect to retrieving the result, not
    /// with respect to launching the operation. That is, the operation will
    /// occur regardless of if the future is ever polled or not, Enabling
    /// a "fire and forget" programming model.
    ///
    /// # Examples
    ///
    ///```
    /// use lamellar::array::prelude::*;
    ///
    /// let world = LamellarWorldBuilder::new().build();
    /// let array = AtomicArray::<usize>::new(&world,100,Distribution::Block);
    ///
    /// let req = array.load(53);
    /// let val = array.block_on(req);
    ///```
    #[tracing::instrument(skip_all)]
    fn load<'a>(&self, index: usize) -> Pin<Box<dyn Future<Output = T> + Send>> {
        let dummy_val = self.inner_array().dummy_val(); //we dont actually do anything with this except satisfy apis;
                                                        // let array = self.inner_array();
        let result = self.inner_array().initiate_batch_fetch_op_2(
            dummy_val,
            index,
            ArrayOpCmd::Load,
            self.as_lamellar_byte_array(),
        );
        Box::pin(async move { result.await[0] })
    }

    /// This call performs a batched vesion of the [load][ReadOnlyOps::load] function,
    /// return a vector of values rather than a single value.
    ///
    /// Instead of a single index, this function expects a list of indicies to load
    /// (See the [OpInput] documentation for a description of valid input containers)
    ///
    /// A future is returned as the result of this call, which is used to retrieve
    /// the results after the (possibly remote) operations have finished.
    ///
    /// # Note
    /// This future is only lazy with respect to retrieving the result, not
    /// with respect to launching the operation. That is, the operation will
    /// occur regardless of if the future is ever polled or not, Enabling
    /// a "fire and forget" programming model.
    ///
    /// # Examples
    ///
    ///```
    /// use lamellar::array::prelude::*;
    ///
    /// let world = LamellarWorldBuilder::new().build();
    /// let array = AtomicArray::<usize>::new(&world,100,Distribution::Block);
    ///
    /// let indices = vec![3,54,12,88,29,68];
    /// let req = array.batch_load(indices.clone());
    /// let vals = array.block_on(req);
    /// assert_eq!(vals.len(),indices.len());
    ///```
    #[tracing::instrument(skip_all)]
    fn batch_load<'a>(
        &self,
        index: impl OpInput<'a, usize>,
    ) -> Pin<Box<dyn Future<Output = Vec<T>> + Send>> {
        let dummy_val = self.inner_array().dummy_val(); //we dont actually do anything with this except satisfy apis;
        self.inner_array().initiate_batch_fetch_op_2(
            dummy_val,
            index,
            ArrayOpCmd::Load,
            self.as_lamellar_byte_array(),
        )
    }
}
