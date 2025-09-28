use libp2p::request_response;
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type ApplyCodec = ByteCodec;
pub type HandshakeCodec = ByteCodec;

#[derive(Debug, Clone, Default)]
pub struct ByteCodec;

#[async_trait::async_trait]
impl request_response::Codec for ByteCodec {
	type Protocol = &'static str;
	type Request = Vec<u8>;
	type Response = Vec<u8>;

	async fn read_request<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
	) -> std::io::Result<Self::Request>
	where
		T: AsyncRead + Unpin + Send,
	{
		let mut len_buf = [0u8; 4];
		io.read_exact(&mut len_buf).await?;
		let len = u32::from_be_bytes(len_buf) as usize;

		let mut buf = vec![0u8; len];
		io.read_exact(&mut buf).await?;
		Ok(buf)
	}

	async fn read_response<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
	) -> std::io::Result<Self::Response>
	where
		T: AsyncRead + Unpin + Send,
	{
		let mut len_buf = [0u8; 4];
		io.read_exact(&mut len_buf).await?;
		let len = u32::from_be_bytes(len_buf) as usize;

		let mut buf = vec![0u8; len];
		io.read_exact(&mut buf).await?;
		Ok(buf)
	}

	async fn write_request<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
		req: Self::Request,
	) -> std::io::Result<()>
	where
		T: AsyncWrite + Unpin + Send,
	{
		let len = req.len() as u32;
		io.write_all(&len.to_be_bytes()).await?;
		io.write_all(&req).await?;
		Ok(())
	}

	async fn write_response<T>(
		&mut self,
		_: &Self::Protocol,
		io: &mut T,
		res: Self::Response,
	) -> std::io::Result<()>
	where
		T: AsyncWrite + Unpin + Send,
	{
		let len = res.len() as u32;
		io.write_all(&len.to_be_bytes()).await?;
		io.write_all(&res).await?;
		Ok(())
	}
}