#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use candle::{DType, Device, IndexOp, Result, Tensor, D};
use candle_nn::VarBuilder;
use text::Qwen3VLTextModel;
use vision::Qwen3VLVisionModel;

pub mod config;
mod conv3d_temporal_2;
mod text;
mod vision;

pub use config::Config;

use crate::models::deepseek2::NonZeroOp;

/// AUDIT-TEMPORARY (diagnostic-only; never shipped): print shape/dtype/
/// min/max/mean/std for a checkpoint tensor, for hand comparison against a
/// Python reference oracle. Controlled by the `QWEN3_VL_AUDIT_DUMP` env var
/// so normal builds/tests are silent.
pub fn audit_dump(label: &str, tensor: &Tensor) {
    if std::env::var("QWEN3_VL_AUDIT_DUMP").is_err() {
        return;
    }
    let Ok(values) = tensor
        .flatten_all()
        .and_then(|t| t.to_dtype(DType::F32))
        .and_then(|t| t.to_vec1::<f32>())
    else {
        eprintln!(
            "[audit] {label}: shape={:?} dtype={:?} (could not flatten to f32)",
            tensor.dims(),
            tensor.dtype()
        );
        return;
    };
    let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean = values.iter().map(|v| *v as f64).sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|v| (*v as f64 - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    let has_nan_inf = values.iter().any(|v| !v.is_finite());
    eprintln!(
        "[audit] {label}: shape={:?} dtype={:?} min={min:.6} max={max:.6} mean={mean:.6} std={:.6} nan_inf={has_nan_inf}",
        tensor.dims(),
        tensor.dtype(),
        variance.sqrt(),
    );
}

/// Mirror of the reference `Qwen3VLModel.get_rope_index` for this runtime's
/// exact usage: batch size 1, at most one image, no video. Text tokens get
/// an identical position value across all three (temporal, height, width)
/// axes; the image span gets genuine per-patch 2D spatial positions. Returns
/// the position ids for `total_len` tokens and the "MRoPE delta" (matching
/// the reference's `mrope_position_deltas`) needed to keep assigning correct
/// positions to text tokens generated after this prefill.
pub fn compute_mrope_position_ids(
    total_len: usize,
    image_span: Option<(usize, usize)>,
    image_grid_thw: Option<[usize; 3]>,
    spatial_merge_size: usize,
) -> (Vec<[i64; 3]>, i64) {
    let mut out = Vec::with_capacity(total_len);
    let mut current_pos: i64 = 0;
    match (image_span, image_grid_thw) {
        (Some((start, end)), Some(grid)) => {
            for _ in 0..start {
                out.push([current_pos; 3]);
                current_pos += 1;
            }
            let llm_t = grid[0];
            let llm_h = grid[1] / spatial_merge_size;
            let llm_w = grid[2] / spatial_merge_size;
            for t in 0..llm_t {
                for h in 0..llm_h {
                    for w in 0..llm_w {
                        out.push([
                            current_pos + t as i64,
                            current_pos + h as i64,
                            current_pos + w as i64,
                        ]);
                    }
                }
            }
            current_pos += llm_h.max(llm_w) as i64;
            for _ in end..total_len {
                out.push([current_pos; 3]);
                current_pos += 1;
            }
        }
        _ => {
            for _ in 0..total_len {
                out.push([current_pos; 3]);
                current_pos += 1;
            }
        }
    }
    let max_pos = out
        .iter()
        .flat_map(|p| p.iter())
        .copied()
        .max()
        .unwrap_or(-1);
    let delta = max_pos + 1 - total_len as i64;
    (out, delta)
}

pub struct Qwen3VLModel {
    text: Qwen3VLTextModel,
    vision: Qwen3VLVisionModel,
}

impl Qwen3VLModel {
    pub fn new(cfg: &Config, vb: VarBuilder) -> Result<Self> {
        let vision = Qwen3VLVisionModel::new(&cfg.vision_config, vb.pp("model").pp("visual"))?;
        let text = Qwen3VLTextModel::new(&cfg.text_config, vb.clone())?;
        Ok(Self { text, vision })
    }

    fn prepare_decoder_attention_mask(
        &self,
        b_size: usize,
        tgt_len: usize,
        seqlen_offset: usize,
        dtype: DType,
        device: &Device,
    ) -> Result<Tensor> {
        let mask: Vec<_> = (0..tgt_len)
            .flat_map(|i| (0..tgt_len).map(move |j| if i < j { f32::NEG_INFINITY } else { 0f32 }))
            .collect();
        let mask = Tensor::from_slice(&mask, (tgt_len, tgt_len), device)?;
        let mask = if seqlen_offset > 0 {
            let mask0 = Tensor::zeros((tgt_len, seqlen_offset), DType::F32, device)?;
            Tensor::cat(&[&mask0, &mask], D::Minus1)?
        } else {
            mask
        };
        mask.expand((
            b_size,
            self.text.num_attn_heads,
            tgt_len,
            tgt_len + seqlen_offset,
        ))?
        .to_dtype(dtype)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn forward(
        &self,
        input_ids: &Tensor,
        pixel_values: Option<Tensor>,
        pixel_values_videos: Option<Tensor>,
        image_grid_thw: Option<Tensor>,
        video_grid_thw: Option<Tensor>,
        seqlens: Vec<usize>,
        continuous_img_pad: Vec<Vec<(usize, usize)>>,
        continuous_vid_pad: Vec<Vec<(usize, usize)>>,
        cache_offset: usize,
        position_ids: &[[i64; 3]],
    ) -> Result<Tensor> {
        let (bs, seqlen) = input_ids.dims2()?;
        debug_assert_eq!(
            position_ids.len(),
            seqlen,
            "one MRoPE [t,h,w] position triple is required per input token"
        );
        // A prefill call presents several NEW query positions at once (the
        // full prompt) and must be causally masked against each other, or
        // every position attends bidirectionally across the whole prompt —
        // corrupting every downstream representation regardless of how
        // correct positional encoding is. A single incremental-decode query
        // position needs no explicit mask: the KV cache already contains
        // only past tokens, so there is nothing "future" to hide.
        let attention_mask = if seqlen > 1 {
            Some(self.prepare_decoder_attention_mask(
                bs,
                seqlen,
                cache_offset,
                self.text.dtype,
                input_ids.device(),
            )?)
        } else {
            None
        };

        let mut input_embeds = self.text.embed_tokens(input_ids)?;
        let (batch_size, seq_len, hidden_dim) = input_embeds.dims3()?;
        let device = input_embeds.device().clone();

        let mut image_mask_opt: Option<Tensor> = None;
        let mut video_mask_opt: Option<Tensor> = None;
        let mut deepstack_image_opt: Option<Vec<Tensor>> = None;
        let mut deepstack_video_opt: Option<Vec<Tensor>> = None;

        if let Some(pixel_values) = &pixel_values {
            let Some(image_grid_thw_ref) = image_grid_thw.as_ref() else {
                candle::bail!("pixel_values require image_grid_thw");
            };
            let mut pixel_values = pixel_values.clone();
            let dims = pixel_values.dims();
            if dims.len() == 3 {
                pixel_values = pixel_values.reshape((dims[0] * dims[1], dims[2]))?;
            }
            let (image_embeds, deepstack_image_embeds) =
                self.vision.forward(&pixel_values, image_grid_thw_ref)?;
            let image_embeds = image_embeds.to_device(&device)?.to_dtype(self.text.dtype)?;
            let mut deepstack_image_embeds = deepstack_image_embeds
                .into_iter()
                .map(|t| t.to_device(&device)?.to_dtype(self.text.dtype))
                .collect::<Result<Vec<_>>>()?;

            let mut offset = 0usize;
            let mut image_mask =
                Tensor::zeros((batch_size, seq_len), DType::F32, input_ids.device())?;
            let total_expected: usize = continuous_img_pad
                .iter()
                .flat_map(|spans| spans.iter().map(|(s, e)| e - s))
                .sum();
            if image_embeds.dim(0)? != total_expected {
                candle::bail!(
                    "Image embedding length {} does not match placeholder tokens {}",
                    image_embeds.dim(0)?,
                    total_expected
                );
            }

            for (batch, spans) in continuous_img_pad.iter().enumerate() {
                for &(start, end) in spans {
                    let len = end - start;
                    let chunk = image_embeds.narrow(0, offset, len)?;
                    offset += len;
                    input_embeds = input_embeds.slice_assign(
                        &[batch..batch + 1, start..end, 0..hidden_dim],
                        &chunk.unsqueeze(0)?,
                    )?;
                    let ones = Tensor::ones((1, len), DType::F32, input_ids.device())?;
                    image_mask = image_mask.slice_assign(&[batch..batch + 1, start..end], &ones)?;
                }
            }
            image_mask_opt = Some(image_mask.to_dtype(DType::U8)?);
            deepstack_image_opt = Some(std::mem::take(&mut deepstack_image_embeds));
        }

        if let Some(pixel_values_videos) = &pixel_values_videos {
            let Some(video_grid_thw_ref) = video_grid_thw.as_ref() else {
                candle::bail!("pixel_values_videos require video_grid_thw");
            };
            let mut pixel_values = pixel_values_videos.clone();
            let dims = pixel_values.dims();
            if dims.len() == 3 {
                pixel_values = pixel_values.reshape((dims[0] * dims[1], dims[2]))?;
            }
            let (video_embeds, deepstack_video_embeds) =
                self.vision.forward(&pixel_values, video_grid_thw_ref)?;
            let video_embeds = video_embeds.to_device(&device)?.to_dtype(self.text.dtype)?;
            let mut deepstack_video_embeds = deepstack_video_embeds
                .into_iter()
                .map(|t| t.to_device(&device)?.to_dtype(self.text.dtype))
                .collect::<Result<Vec<_>>>()?;

            let mut offset = 0usize;
            let mut video_mask =
                Tensor::zeros((batch_size, seq_len), DType::F32, input_ids.device())?;
            let total_expected: usize = continuous_vid_pad
                .iter()
                .flat_map(|spans| spans.iter().map(|(s, e)| e - s))
                .sum();
            if video_embeds.dim(0)? != total_expected {
                candle::bail!(
                    "Video embedding length {} does not match placeholder tokens {}",
                    video_embeds.dim(0)?,
                    total_expected
                );
            }

            for (batch, spans) in continuous_vid_pad.iter().enumerate() {
                for &(start, end) in spans {
                    let len = end - start;
                    let chunk = video_embeds.narrow(0, offset, len)?;
                    offset += len;
                    input_embeds = input_embeds.slice_assign(
                        &[batch..batch + 1, start..end, 0..hidden_dim],
                        &chunk.unsqueeze(0)?,
                    )?;
                    let ones = Tensor::ones((1, len), DType::F32, input_ids.device())?;
                    video_mask = video_mask.slice_assign(&[batch..batch + 1, start..end], &ones)?;
                }
            }
            video_mask_opt = Some(video_mask.to_dtype(DType::U8)?);
            deepstack_video_opt = Some(std::mem::take(&mut deepstack_video_embeds));
        }

        let (visual_pos_masks, deepstack_visual_embeds) = match (
            image_mask_opt,
            deepstack_image_opt,
            video_mask_opt,
            deepstack_video_opt,
        ) {
            (Some(image_mask), Some(image_deepstack), Some(video_mask), Some(video_deepstack)) => {
                let combined =
                    (image_mask.to_dtype(DType::F32)? + video_mask.to_dtype(DType::F32)?)?;
                let visual_mask = combined.gt(0f32)?.to_dtype(DType::U8)?;
                let visual_indices = visual_mask.flatten_all()?.nonzero()?.squeeze(1)?;
                let visual_indices_vec = visual_indices.to_vec1::<i64>()?;

                let image_flat = image_mask
                    .flatten_all()?
                    .to_dtype(DType::U8)?
                    .to_vec1::<u8>()?;
                let num_visual = visual_indices_vec.len();
                if image_deepstack.len() != video_deepstack.len() {
                    candle::bail!(
                        "DeepStack image layers ({}) do not match video layers ({})",
                        image_deepstack.len(),
                        video_deepstack.len()
                    );
                }
                let mut combined_layers = Vec::with_capacity(image_deepstack.len());
                for (img_layer, vid_layer) in image_deepstack.iter().zip(video_deepstack.iter()) {
                    let mut rows = Vec::with_capacity(num_visual);
                    let mut img_offset = 0usize;
                    let mut vid_offset = 0usize;
                    for &idx in &visual_indices_vec {
                        let idx = idx as usize;
                        if image_flat[idx] != 0 {
                            rows.push(img_layer.i(img_offset)?);
                            img_offset += 1;
                        } else {
                            rows.push(vid_layer.i(vid_offset)?);
                            vid_offset += 1;
                        }
                    }
                    if img_offset != img_layer.dim(0)? || vid_offset != vid_layer.dim(0)? {
                        candle::bail!(
                                "DeepStack feature alignment failed for images ({}/{}) or videos ({}/{})",
                                img_offset,
                                img_layer.dim(0)?,
                                vid_offset,
                                vid_layer.dim(0)?
                            );
                    }
                    let row_refs: Vec<&Tensor> = rows.iter().collect();
                    combined_layers.push(Tensor::stack(&row_refs, 0)?);
                }
                (Some(visual_mask), Some(combined_layers))
            }
            (Some(image_mask), Some(image_deepstack), _, _) => {
                (Some(image_mask), Some(image_deepstack))
            }
            (_, _, Some(video_mask), Some(video_deepstack)) => {
                (Some(video_mask), Some(video_deepstack))
            }
            _ => (None, None),
        };

        let mut ropeidx_attn_mask_bs = Vec::new();
        let max_seqlens = *seqlens.iter().max().unwrap();
        for len in &seqlens {
            ropeidx_attn_mask_bs.push(Tensor::new(
                [vec![1f32; *len], vec![0f32; max_seqlens - len]].concat(),
                input_ids.device(),
            )?);
        }

        let out = self.text.forward_embeds(
            input_embeds,
            attention_mask.as_ref(),
            position_ids,
            visual_pos_masks.as_ref(),
            deepstack_visual_embeds.as_deref(),
        )?;
        Ok(out)
    }
}
