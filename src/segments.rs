//! 依据电位差分切分正扫/反扫。
//!
//! 关键不变量（针对“连续相同电位平台不能被导数符号随机拆圈、
//! 电位回转点只属于其中一个扫描段”）：
//!
//! 1. 标签集合只有 `+1`（正扫/电位递增）、`-1`（反扫/电位递减）。
//! 2. 平台点（`|ΔV| <= tol`）继承“最近一个明确方向”，因此一整段
//!    水平平台（包括把回转平台再延长一个样本）得到同一个标签，
//!    不会因差分符号抖动被拆成多段。
//! 3. 真正的方向反转：第一个明确朝新方向走的样本开启新段；
//!    回转点（电位极值点，以及极值处的整个等电位平台）全部保留在
//!    前一段内，即“回转点只属于其中一个扫描段”。段索引区间互不重叠。
//! 4. 若数据以平台开始（开头尚无方向），整个起始平台沿用第一个
//!    明确方向：起始平台归属于第一段。
//!
//! `cycle` 按扫描方向交替计数：第一段为 cycle 1，每次方向变化 cycle+1。

use crate::model::{Sample, Segment, SegmentKind};

pub fn segment_samples(samples: &[Sample], tol_v: f64) -> (Vec<i8>, Vec<Segment>) {
    let n = samples.len();
    let mut labels = vec![0i8; n];
    if n == 0 {
        return (labels, Vec::new());
    }

    // 第一遍：从左到右，平台继承最近的明确方向；
    // 但起始平台（前面无方向）记 0，稍后用首个方向回填。
    let mut current: i8 = 0;
    for i in 0..n {
        if i == 0 {
            continue; // label[0] 后面由邻点确定
        }
        let dv = samples[i].potential_v - samples[i - 1].potential_v;
        if dv > tol_v {
            current = 1;
        } else if dv < -tol_v {
            current = -1;
        }
        labels[i] = current;
    }
    // 回填起始平台 + 样本 0
    let first_dir = labels.iter().copied().find(|&l| l != 0).unwrap_or(1);
    for l in labels.iter_mut() {
        if *l == 0 {
            *l = first_dir;
        }
    }

    // 第二遍：把标签序列收敛成极大同号游程。由于平台继承规则，
    // 同一极值平台内的标签已经一致，回转点天然只落在一个段。
    let mut segments: Vec<Segment> = Vec::new();
    let mut start = 0usize;
    let mut dir = labels[0];
    for i in 1..n {
        if labels[i] != dir {
            segments.push(make_segment(dir, start, i - 1, segments.len()));
            start = i;
            dir = labels[i];
        }
    }
    segments.push(make_segment(dir, start, n - 1, segments.len()));

    // cycle 按方向变化次数编号
    let mut cycle = 1i64;
    let mut prev_kind: Option<SegmentKind> = None;
    for seg in &mut segments {
        if let Some(prev) = prev_kind {
            if seg.kind != prev {
                cycle += 1;
            }
        }
        seg.cycle = cycle;
        prev_kind = Some(seg.kind);
    }

    (labels, segments)
}

fn make_segment(dir: i8, start: usize, end: usize, seg_index: usize) -> Segment {
    let kind = if dir < 0 { SegmentKind::Reverse } else { SegmentKind::Forward };
    let _ = seg_index;
    Segment { kind, cycle: 1, start_idx: start, end_idx: end }
}

/// 返回样本所属段下标（供溯源与区间归属校验）。
pub fn segment_index_at(segments: &[Segment], idx: usize) -> Option<usize> {
    segments
        .iter()
        .position(|s| idx >= s.start_idx && idx <= s.end_idx)
}

/// 校验区间端点落在同一段内。
pub fn bounds_in_same_segment(segments: &[Segment], lo: usize, hi: usize) -> bool {
    let a = segment_index_at(segments, lo);
    let b = segment_index_at(segments, hi);
    a.zip(b).map(|(x, y)| x == y).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(pot: &[f64]) -> Vec<Sample> {
        pot.iter()
            .enumerate()
            .map(|(i, &p)| Sample { index: i, potential_v: p, current_a: 0.0, time_s: i as f64 })
            .collect()
    }

    #[test]
    fn simple_triangle_two_segments() {
        let s = mk(&[0.0, 0.1, 0.2, 0.3, 0.2, 0.1, 0.0]);
        let (labels, segs) = segment_samples(&s, 1e-9);
        assert_eq!(labels, vec![1, 1, 1, 1, -1, -1, -1]);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].kind, SegmentKind::Forward);
        assert_eq!(segs[1].kind, SegmentKind::Reverse);
        // 回转点（index 3）只属于第一段
        assert!(segs[0].contains(3));
        assert!(!segs[1].contains(3));
        assert_eq!(segs[1].start_idx, 4);
    }

    #[test]
    fn reversal_plateau_extended_by_one_sample_does_not_jitter() {
        // 正扫到 0.3，极值平台延长一个样本（两个 0.3），然后反扫
        let s = mk(&[0.0, 0.1, 0.2, 0.3, 0.3, 0.3, 0.2, 0.1, 0.0]);
        let (labels, segs) = segment_samples(&s, 1e-9);
        // 整段 0.3 平台都继承正扫方向
        assert_eq!(labels[3], 1);
        assert_eq!(labels[4], 1);
        assert_eq!(labels[5], 1);
        assert_eq!(labels[6], -1);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].end_idx == 5);
        assert!(segs[1].start_idx == 6);
        // 段不重叠、不缺漏
        assert_eq!(segs[0].end_idx + 1, segs[1].start_idx);

        // 再延长一个样本，结论不变（不抖动）
        let s2 = mk(&[0.0, 0.1, 0.2, 0.3, 0.3, 0.3, 0.3, 0.2, 0.1, 0.0]);
        let (l2, segs2) = segment_samples(&s2, 1e-9);
        assert_eq!(segs2.len(), 2);
        assert_eq!(&l2[3..7], &[1, 1, 1, 1]);
        assert_eq!(segs2[0].end_idx, 6);
        assert_eq!(segs2[1].start_idx, 7);
    }

    #[test]
    fn mid_scan_plateau_keeps_direction() {
        // 扫描中间出现等电位平台，不能把一段拆开
        let s = mk(&[0.0, 0.1, 0.1, 0.1, 0.2, 0.3]);
        let (_labels, segs) = segment_samples(&s, 1e-9);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].start_idx, 0);
        assert_eq!(segs[0].end_idx, 5);
    }

    #[test]
    fn tolerance_absorbs_noise_plateau() {
        let s = mk(&[0.0, 0.1, 0.10000001, 0.09999999, 0.2, 0.3]);
        let (_l, segs) = segment_samples(&s, 1e-6);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn leading_platform_belongs_to_first_segment() {
        let s = mk(&[0.0, 0.0, 0.1, 0.2, 0.3]);
        let (labels, segs) = segment_samples(&s, 1e-9);
        assert_eq!(labels[0], 1);
        assert_eq!(labels[1], 1);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn full_cycle_with_plateaus_segments_are_contiguous_partition() {
        let mut pot = vec![0.0, 0.0, 0.1, 0.2, 0.3, 0.3, 0.3, 0.2, 0.1, 0.0, 0.0, 0.0];
        let _ = &mut pot;
        let s = mk(&pot);
        let (_l, segs) = segment_samples(&s, 1e-9);
        assert_eq!(segs.first().unwrap().start_idx, 0);
        assert_eq!(segs.last().unwrap().end_idx, s.len() - 1);
        for w in segs.windows(2) {
            assert_eq!(w[0].end_idx + 1, w[1].start_idx, "段必须相邻且不重叠");
        }
        assert!(segs.iter().all(|x| x.end_idx >= x.start_idx));
    }
}
