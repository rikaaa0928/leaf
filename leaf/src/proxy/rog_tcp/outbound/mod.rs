#[cfg(feature = "outbound-rog-tcp")]
pub mod datagram;
#[cfg(feature = "outbound-rog-tcp")]
pub mod stream;

#[cfg(feature = "outbound-rog-tcp")]
pub use datagram::Handler as DatagramHandler;
#[cfg(feature = "outbound-rog-tcp")]
pub use stream::Handler as StreamHandler;
