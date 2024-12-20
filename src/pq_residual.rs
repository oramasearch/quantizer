use crate::errors::PQResidualError;
use crate::pq::{CodeType, PQ};
use ndarray::{s, Array2, Array3, ArrayView2, Axis};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub enum Codes3D {
    U8(Array3<u8>),
    U16(Array3<u16>),
    U32(Array3<u32>),
}

pub struct PQResidual {
    deep: usize,
    code_dtype: CodeType,
    m: usize,
    pqs: Vec<PQ>,
}

#[derive(Default)]
pub struct SaveOptions {
    pub save_codebook: bool,
    pub save_decoded: Vec<usize>,
    pub save_residue_norms: Vec<usize>,
    pub save_results_t: bool,
    pub dataset_name: String,
    pub save_dir: PathBuf,
}

fn compute_norms(data: &ArrayView2<f32>) -> Vec<f32> {
    data.rows()
        .into_iter()
        .map(|row| row.dot(&row).sqrt())
        .collect()
}

fn save_norms(norms: &[f32], path: &str) -> Result<(), io::Error> {
    let mut file = File::create(path)?;
    for norm in norms {
        file.write_all(&norm.to_le_bytes())?;
    }
    Ok(())
}

fn save_decoded_data(data: &Array2<f32>, path: &str) -> Result<(), io::Error> {
    let mut file = File::create(path)?;
    for &val in data.iter() {
        file.write_all(&val.to_le_bytes())?;
    }
    Ok(())
}

impl PQResidual {
    pub fn try_new(pqs: Vec<PQ>) -> Result<Self, PQResidualError> {
        if pqs.is_empty() {
            return Err(PQResidualError::MissingProductQuantizer);
        };

        let m = pqs
            .iter()
            .map(|pq| pq.m)
            .max()
            .ok_or(PQResidualError::MissingProductQuantizer)?;

        let code_dtype = pqs
            .get(0)
            .ok_or(PQResidualError::MissingProductQuantizer)?
            .code_dtype;

        Ok(PQResidual {
            deep: pqs.len(),
            code_dtype,
            pqs,
            m,
        })
    }

    pub fn fit(
        &mut self,
        t: &Array2<f32>,
        iter: usize,
        save_codebook: bool,
        save_decoded: &[usize],
        save_residue_norms: &[usize],
        save_results_t: bool,
        dataset_name: Option<&str>,
        save_dir: Option<&Path>,
        d: Option<&Array2<f32>>,
    ) -> Result<(), PQResidualError> {
        let save_dir: PathBuf = save_dir
            .unwrap_or_else(|| Path::new("./results"))
            .to_path_buf();

        let ks = self
            .pqs
            .get(0)
            .ok_or(PQResidualError::MissingProductQuantizer)?
            .ks;

        let mut vecs = t.clone();
        let mut vecs_d = d.map(|data| data.clone());

        let mut codebook_file = if save_codebook {
            let dataset_name = dataset_name.ok_or(PQResidualError::MissingDatasetName)?;
            let file_name = format!("{}_rq_{}_{}_codebook", dataset_name, self.deep, ks);
            let file_path = save_dir.join(file_name);
            Some(File::create(file_path)?)
        } else {
            None
        };

        for (layer, pq) in self.pqs.iter_mut().enumerate() {
            pq.fit(&vecs, iter)?;

            let compressed = pq.compress(&vecs)?;
            vecs -= &compressed;

            if let Some(ref mut vecs_d) = vecs_d {
                let compressed_d = pq.compress(vecs_d)?;
                *vecs_d -= &compressed_d;
            }

            if log::log_enabled!(log::Level::Info) {
                let norms: Vec<f32> = vecs
                    .axis_iter(Axis(0))
                    .map(|row| row.dot(&row).sqrt())
                    .collect();
                let mean_norm = norms.iter().copied().sum::<f32>() / norms.len() as f32;
                let max_norm = norms.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let min_norm = norms.iter().cloned().fold(f32::INFINITY, f32::min);
                println!(
                    "# layer: {}, residual average norm: {}, max norm: {}, min norm: {}",
                    layer, mean_norm, max_norm, min_norm
                );
            }

            if save_residue_norms.contains(&(layer + 1)) {
                let dataset_name = dataset_name.ok_or(PQResidualError::MissingDatasetName)?;
                let file_name = format!("{}_rq_{}_{}_residue_norms", dataset_name, layer + 1, ks);
                let file_path = save_dir.join(file_name);
                let mut f = File::create(file_path)?;

                if save_results_t {
                    for norm in vecs.axis_iter(Axis(0)).map(|row| row.dot(&row).sqrt()) {
                        f.write_all(&norm.to_le_bytes())?;
                    }
                }
                if let Some(ref vecs_d) = vecs_d {
                    for norm in vecs_d.axis_iter(Axis(0)).map(|row| row.dot(&row).sqrt()) {
                        f.write_all(&norm.to_le_bytes())?;
                    }
                }
            }

            if save_decoded.contains(&(layer + 1)) {
                let dataset_name = dataset_name.ok_or(PQResidualError::MissingDatasetName)?;
                let file_name = format!("{}_rq_{}_{}_decoded", dataset_name, layer + 1, ks);
                let file_path = save_dir.join(file_name);
                let mut f = File::create(file_path)?;

                if save_results_t {
                    let decoded = t - &vecs;
                    for row in decoded.axis_iter(Axis(0)) {
                        for val in row.iter() {
                            f.write_all(&val.to_le_bytes())?;
                        }
                    }
                }
                if let Some(ref vecs_d) = vecs_d {
                    let D = d.ok_or(PQResidualError::MissingProductQuantizer)?;
                    let decoded_d = D - vecs_d;
                    for row in decoded_d.axis_iter(Axis(0)) {
                        for val in row.iter() {
                            f.write_all(&val.to_le_bytes())?;
                        }
                    }
                }
            }

            if let Some(ref mut codebook_f) = codebook_file {
                let codewords = pq
                    .codewords
                    .as_ref()
                    .ok_or(PQResidualError::ModelNotTrained)?;
                for val in codewords.iter() {
                    codebook_f.write_all(&val.to_le_bytes())?;
                }
            }
        }

        Ok(())
    }

    pub fn encode(&self, vecs: &Array2<f32>) -> Result<Codes3D, PQResidualError> {
        let (n, _d) = vecs.dim();
        let mut residual_vecs = vecs.clone();

        match self.code_dtype {
            CodeType::U8 => {
                let mut codes = Array3::<u8>::zeros((n, self.deep, self.m));
                for (i, pq) in self.pqs.iter().enumerate() {
                    let pq_m = pq.m;
                    let pq_codes_u32 = pq.encode(&residual_vecs)?;
                    let pq_codes = pq_codes_u32.map(|&x| x as u8);

                    codes.slice_mut(s![.., i, 0..pq_m]).assign(&pq_codes);

                    let reconstructed = pq.decode(&pq_codes_u32)?;
                    residual_vecs -= &reconstructed;
                }
                Ok(Codes3D::U8(codes))
            }
            CodeType::U16 => {
                let mut codes = Array3::<u16>::zeros((n, self.deep, self.m));
                for (i, pq) in self.pqs.iter().enumerate() {
                    let pq_m = pq.m;
                    let pq_codes_u32 = pq.encode(&residual_vecs)?;
                    let pq_codes = pq_codes_u32.map(|&x| x as u16);

                    codes.slice_mut(s![.., i, 0..pq_m]).assign(&pq_codes);

                    let reconstructed = pq.decode(&pq_codes_u32)?;
                    residual_vecs -= &reconstructed;
                }
                Ok(Codes3D::U16(codes))
            }
            CodeType::U32 => {
                let mut codes = Array3::<u32>::zeros((n, self.deep, self.m));
                for (i, pq) in self.pqs.iter().enumerate() {
                    let pq_m = pq.m;
                    let pq_codes = pq.encode(&residual_vecs)?;

                    codes.slice_mut(s![.., i, 0..pq_m]).assign(&pq_codes);

                    let reconstructed = pq.decode(&pq_codes)?;
                    residual_vecs -= &reconstructed;
                }
                Ok(Codes3D::U32(codes))
            }
        }
    }

    pub fn decode(&self, codes: &Array3<u32>) -> Result<Array2<f32>, PQResidualError> {
        let (n, deep, m) = codes.dim();

        if deep != self.deep {
            return Err(PQResidualError::MissingProductQuantizer);
        }

        if self.pqs.is_empty() {
            return Err(PQResidualError::MissingProductQuantizer);
        }

        let dimension = self.pqs[0].dim.ok_or(PQResidualError::ModelNotTrained)?;

        let mut sum_vecs = Array2::<f32>::zeros((n, dimension));

        for (i, pq) in self.pqs.iter().enumerate() {
            let pq_m = pq.m;
            if pq_m > m {
                return Err(PQResidualError::MissingProductQuantizer);
            }

            let codes_slice = codes.slice(s![.., i, 0..pq_m]).to_owned();
            let decoded = pq.decode(&codes_slice)?;
            sum_vecs = sum_vecs + &decoded;
        }

        Ok(sum_vecs)
    }

    pub fn compress(&self, X: &Array2<f32>) -> Result<Array2<f32>, PQResidualError> {
        let (n, d) = X.dim();
        let mut sum_residual = Array2::<f32>::zeros((n, d));
        let mut vecs = X.clone();

        for pq in &self.pqs {
            let compressed = pq.compress(&vecs)?;
            vecs = &vecs - &compressed;
            sum_residual = sum_residual + &compressed;
        }

        Ok(sum_residual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pq::{CodeType, PQ};
    use ndarray::Array2;

    fn create_dummy_pq(m: usize, ks: u32) -> PQ {
        PQ::try_new(m, ks).unwrap()
    }

    #[test]
    fn test_try_new_empty_pqs() {
        let pqs: Vec<PQ> = vec![];
        let result = PQResidual::try_new(pqs);
        assert!(result.is_err(), "Should fail with empty pqs vector");
    }

    #[test]
    fn test_try_new_single_pq() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let result = PQResidual::try_new(pqs);
        assert!(result.is_ok(), "Should succeed with a single PQ");
        let residual = result.unwrap();
        assert_eq!(residual.deep, 1);
        assert_eq!(residual.m, 4);
        assert_eq!(residual.code_dtype, CodeType::U8);
    }

    #[test]
    fn test_try_new_multiple_pqs() {
        let pqs = vec![create_dummy_pq(4, 4), create_dummy_pq(8, 4)];
        let residual = PQResidual::try_new(pqs).expect("Should succeed with multiple PQs");
        assert_eq!(residual.deep, 2);
        assert_eq!(residual.m, 8);
    }

    #[test]
    fn test_fit_with_small_data() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let mut residual = PQResidual::try_new(pqs).unwrap();
        let data = Array2::<f32>::from_shape_fn((10, 4), |(i, j)| (i * j) as f32);

        let result = residual.fit(
            &data,
            5,
            false,
            &[],
            &[],
            false,
            Some("testdata"),
            None,
            None,
        );

        assert!(
            result.is_ok(),
            "Fit should not fail on valid data with ks=4 and 10 samples"
        );
    }

    #[test]
    fn test_encode_decode_round_trip() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let mut residual = PQResidual::try_new(pqs).unwrap();
        let data = Array2::<f32>::from_shape_fn((20, 4), |(i, j)| (i + j) as f32);

        residual.pqs[0]
            .fit(&data, 5)
            .expect("PQ fit should succeed with ks=4 and 20 samples");

        let codes = residual
            .encode(&data)
            .expect("Encoding should succeed after fit");
        let reconstructed = match codes {
            Codes3D::U8(c) => residual.decode(&c.map(|&x| x as u32)).unwrap(),
            Codes3D::U16(c) => residual.decode(&c.map(|&x| x as u32)).unwrap(),
            Codes3D::U32(c) => residual.decode(&c).unwrap(),
        };

        assert_eq!(
            reconstructed.dim(),
            data.dim(),
            "Decoded data shape mismatch"
        );
    }

    #[test]
    fn test_compress() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let mut residual = PQResidual::try_new(pqs).unwrap();
        let data = Array2::<f32>::from_shape_fn((20, 4), |(i, j)| (i * j) as f32);

        residual.pqs[0]
            .fit(&data, 5)
            .expect("Fit should succeed with ks=4 and 20 samples");

        let compressed = residual
            .compress(&data)
            .expect("Compress should succeed after fit");
        assert_eq!(compressed.dim(), data.dim());
    }

    #[test]
    fn test_decode_error_wrong_dimensions() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let residual = PQResidual::try_new(pqs).unwrap();

        let codes = Array3::<u32>::zeros((10, 2, residual.m));
        let result = residual.decode(&codes);
        assert!(result.is_err(), "Should fail if code depth doesn't match");
    }

    #[test]
    fn test_error_before_fit() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let residual = PQResidual::try_new(pqs).unwrap();
        let data = Array2::<f32>::zeros((10, 4));

        let encode_result = residual.encode(&data);
        assert!(
            encode_result.is_err(),
            "Encode should fail if PQ not fitted"
        );
    }

    #[test]
    fn test_fit_with_zero_samples() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let mut residual = PQResidual::try_new(pqs).unwrap();

        let data = Array2::<f32>::zeros((0, 4));
        let result = residual.fit(
            &data,
            5,
            false,
            &[],
            &[],
            false,
            Some("zero_samples"),
            None,
            None,
        );
        assert!(result.is_err(), "Fit should fail with zero samples");
    }

    #[test]
    fn test_fit_with_zero_dimensions() {
        let pqs = vec![create_dummy_pq(4, 4)];
        let mut residual = PQResidual::try_new(pqs).unwrap();

        let data = Array2::<f32>::zeros((10, 0));
        let result = residual.fit(
            &data,
            5,
            false,
            &[],
            &[],
            false,
            Some("zero_dims"),
            None,
            None,
        );
        assert!(result.is_err(), "Fit should fail with zero dimensions");
    }
}
