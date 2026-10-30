#[cfg(feature = "outbound-rog-h3")]
pub mod datagram;
#[cfg(feature = "outbound-rog-h3")]
pub mod stream;

#[cfg(feature = "outbound-rog-h3")]
pub use datagram::Handler as DatagramHandler;
#[cfg(feature = "outbound-rog-h3")]
pub use stream::Handler as StreamHandler;
