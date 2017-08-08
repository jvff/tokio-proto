//! Multiplexed RPC protocols.
//!
//! See the crate-level docs for an overview.

mod client;
pub use self::client::ClientProto;
pub use self::client::ClientService;
pub use self::client::ClientFuture;

mod server;
pub use self::server::ServerProto;

/// Identifies a request / response thread
pub type RequestId = u64;

/// A marker used to flag protocols as being multiplexed RPC.
///
/// This is an implementation detail; to actually implement a protocol,
/// implement the `ClientProto` or `ServerProto` traits in this module.
#[derive(Debug)]
pub struct Multiplex;

// This is a submodule so that `LiftTransport` can be marked `pub`, to satisfy
// the no-private-in-public checker.
mod lift {
    use std::io;
    use std::marker::PhantomData;

    use super::RequestId;
    use streaming::multiplex::{Frame, Transport};
    use futures::{Future, Stream, Sink, StartSend, Poll, Async, AsyncSink};

    // Lifts an implementation of RPC-style transport to streaming-style transport
    pub struct LiftTransport<T, E>(pub T, pub PhantomData<E>);

    // Lifts the Bind from the underlying transport
    pub struct LiftBind<A, F, E> {
        fut: F,
        marker: PhantomData<(A, E)>,
    }

    impl<T, InnerItem, E> Stream for LiftTransport<T, E> where
        E: 'static,
        T: Stream<Item = (RequestId, InnerItem)>,
        T::Error: From<io::Error>,
    {
        type Item = Frame<InnerItem, (), E>;
        type Error = T::Error;

        fn poll(&mut self) -> Poll<Option<Self::Item>, T::Error> {
            let (id, msg) = match try_ready!(self.0.poll()) {
                Some(msg) => msg,
                None => return Ok(None.into()),
            };
            Ok(Some(Frame::Message {
                message: msg,
                body: false,
                solo: false,
                id: id,
            }).into())
        }
    }

    impl<T, InnerSink, E> Sink for LiftTransport<T, E> where
        E: 'static,
        T: Sink<SinkItem = (RequestId, InnerSink)>,
        T::SinkError: From<io::Error>,
    {
        type SinkItem = Frame<InnerSink, (), E>;
        type SinkError = T::SinkError;

        fn start_send(&mut self, request: Self::SinkItem)
                      -> StartSend<Self::SinkItem, T::SinkError> {
            if let Frame::Message { message, id, body, solo } = request {
                if !body && !solo {
                    match try!(self.0.start_send((id, message))) {
                        AsyncSink::Ready => return Ok(AsyncSink::Ready),
                        AsyncSink::NotReady((id, msg)) => {
                            let msg = Frame::Message {
                                message: msg,
                                id: id,
                                body: false,
                                solo: false,
                            };
                            return Ok(AsyncSink::NotReady(msg))
                        }
                    }
                }
            }
            Err(io::Error::new(io::ErrorKind::Other, "no support for streaming").into())
        }

        fn poll_complete(&mut self) -> Poll<(), T::SinkError> {
            self.0.poll_complete()
        }

        fn close(&mut self) -> Poll<(), T::SinkError> {
            self.0.close()
        }
    }

    impl<T, InnerItem, InnerSink, E> Transport<()> for LiftTransport<T, E> where
        E: 'static + From<io::Error>,
        T: 'static,
        T: Stream<Item = (RequestId, InnerItem), Error = E>,
        T: Sink<SinkItem = (RequestId, InnerSink), SinkError = E>,
    {}

    impl<A, F, E> LiftBind<A, F, E> {
        pub fn lift(f: F) -> LiftBind<A, F, E> {
            LiftBind {
                fut: f,
                marker: PhantomData,
            }
        }
    }

    impl<A, F, E> Future for LiftBind<A, F, E> where
        F: Future<Error = E>,
        E: From<io::Error>,
    {
        type Item = LiftTransport<F::Item, E>;
        type Error = E;

        fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
            Ok(Async::Ready(LiftTransport(try_ready!(self.fut.poll()), PhantomData)))
        }
    }
}
